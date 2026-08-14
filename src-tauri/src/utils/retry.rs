//! 指数退避 + jitter 重试（可恢复错误白名单由调用方传入判断函数）。
//!
//! 用法：
//! ```ignore
//! let out = retry_with_backoff(&STREAM_REQUEST_POLICY, &mut || async { ... },
//!     |e| e.retryable(), |e| e.retry_after_ms()).await;
//! // out.attempts 为实际尝试次数（供任务级 Trace 记录重试开销）
//! ```

use std::future::Future;
use std::time::Duration;

/// 重试策略：总尝试次数 + 指数退避参数
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// 总尝试次数（含首次请求）
    pub max_attempts: usize,
    /// 首次重试延迟基数（毫秒），每次翻倍
    pub base_delay_ms: u64,
    /// 退避上限（毫秒）
    pub max_delay_ms: u64,
}

/// LLM 请求重试（连接 / 状态检查阶段）
pub const STREAM_REQUEST_POLICY: RetryPolicy = RetryPolicy {
    max_attempts: 3,
    base_delay_ms: 1_000,
    max_delay_ms: 15_000,
};

/// 工具执行重试（命令超时 / 网络类，替换原先固定重试一次）
pub const TOOL_POLICY: RetryPolicy = RetryPolicy {
    max_attempts: 3,
    base_delay_ms: 800,
    max_delay_ms: 8_000,
};

/// 重试结果：最后一次尝试的结果 + 实际尝试次数
pub struct RetryResult<T, E> {
    pub value: Result<T, E>,
    /// 实际尝试次数（1..=max_attempts）
    pub attempts: usize,
}

/// 指数退避重试：失败后等待 `base * 2^(n-1)`（封顶 max_delay）+ jitter，
/// 尊重调用方给出的 Retry-After（如 Provider 限流头），但至少等待一档退避。
pub async fn retry_with_backoff<T, E, F, Fut>(
    policy: &RetryPolicy,
    f: &mut F,
    should_retry: impl Fn(&E) -> bool,
    retry_after_of: impl Fn(&E) -> Option<u64>,
) -> RetryResult<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempts = 0usize;
    let mut retry_after: Option<u64> = None;
    loop {
        attempts += 1;
        match f().await {
            Ok(v) => return RetryResult { value: Ok(v), attempts },
            Err(e) => {
                if attempts >= policy.max_attempts || !should_retry(&e) {
                    return RetryResult { value: Err(e), attempts };
                }
                if let Some(ra) = retry_after_of(&e) {
                    retry_after = Some(retry_after.map_or(ra, |cur| cur.max(ra)));
                }
                let exp = policy.base_delay_ms.saturating_mul(1u64 << (attempts - 1).min(6));
                let backoff = exp.min(policy.max_delay_ms);
                let wait = retry_after.map_or(backoff, |ra| ra.max(backoff.min(200)));
                tokio::time::sleep(Duration::from_millis(wait + jitter(wait))).await;
            }
        }
    }
}

/// 轻量 jitter（0..seed/2+1 毫秒）：避免多实例同时重试的惊群效应
fn jitter(seed: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    nanos % (seed.min(1_000) / 2 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 首次失败（可重试）→ 第二次成功：attempts = 2
    #[tokio::test]
    async fn test_retry_then_success() {
        let calls = std::cell::Cell::new(0);
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 5,
            max_delay_ms: 20,
        };
        let out: RetryResult<i32, &str> = retry_with_backoff(
            &policy,
            &mut || async {
                calls.set(calls.get() + 1);
                if calls.get() < 2 {
                    Err("timeout")
                } else {
                    Ok(42)
                }
            },
            |e: &&str| e.contains("timeout"),
            |_| None,
        )
        .await;
        assert_eq!(out.attempts, 2);
        assert_eq!(out.value.unwrap(), 42);
    }

    /// 不可重试错误：立即返回，不重试
    #[tokio::test]
    async fn test_non_retryable_stops() {
        let calls = std::cell::Cell::new(0);
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 5,
            max_delay_ms: 20,
        };
        let out: RetryResult<i32, &str> = retry_with_backoff(
            &policy,
            &mut || async {
                calls.set(calls.get() + 1);
                Err("auth failed")
            },
            |e: &&str| e.contains("timeout"),
            |_| None,
        )
        .await;
        assert_eq!(out.attempts, 1);
        assert!(out.value.is_err());
        assert_eq!(calls.get(), 1);
    }

    /// 重试耗尽：attempts = max_attempts，返回最后一次错误
    #[tokio::test]
    async fn test_exhausted() {
        let calls = std::cell::Cell::new(0);
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 2,
            max_delay_ms: 10,
        };
        let out: RetryResult<i32, &str> = retry_with_backoff(
            &policy,
            &mut || async {
                calls.set(calls.get() + 1);
                Err("network down")
            },
            |e: &&str| e.contains("network"),
            |_| None,
        )
        .await;
        assert_eq!(out.attempts, 3);
        assert!(out.value.is_err());
        assert_eq!(calls.get(), 3);
    }

    /// 尊重 Retry-After：两次失败各等约 60ms（jitter 只增不减，耗时 ≥ 100ms）
    #[tokio::test]
    async fn test_retry_after_respected() {
        let calls = std::cell::Cell::new(0);
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 5,
            max_delay_ms: 10,
        };
        let started = std::time::Instant::now();
        let out: RetryResult<String, Option<u64>> = retry_with_backoff(
            &policy,
            &mut || async {
                calls.set(calls.get() + 1);
                Err(Some(60u64))
            },
            |_| true,
            |ra: &Option<u64>| *ra,
        )
        .await;
        assert_eq!(calls.get(), 3);
        let elapsed = started.elapsed().as_millis();
        assert!(elapsed >= 100, "elapsed={elapsed}");
        assert!(out.value.is_err());
    }

    #[test]
    fn test_jitter_in_range() {
        for seed in [10u64, 100, 1000] {
            let j = jitter(seed);
            assert!(j <= seed.min(1000) / 2, "seed={seed} jitter={j}");
        }
    }
}
