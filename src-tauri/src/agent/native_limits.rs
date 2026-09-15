//! 原生沙箱的资源限制：把 spec 里**语义等价**的限额落到子进程 rlimit 上。
//!
//! 只有内核真正执行、且语义与 spec 一致的限额才声明为「已应用」：
//! - `cpu_seconds` → `RLIMIT_CPU`：每进程 CPU 时间上限，超限由内核终止（SIGXCPU/SIGKILL）。
//! - `memory_mb` → `RLIMIT_AS`：地址空间上限。**macOS 内核不接受该限额**（setrlimit 返回
//!   EINVAL），因此用运行时探测决定是否声明已应用，探测失败就如实标注未限制。
//!
//! 不能表达的限额不会被伪装成已限制（`platform_gaps` 给出可展示的原因）：
//! - `pids`：`RLIMIT_NPROC` 按**用户全局计数**（实测软限设到 40 后子进程连 fork 都失败），
//!   与「容器内进程数」语义不同，映射过去只会让命令不可用。
//! - `cpu_count`：CPU 配额需要 cgroups，rlimit 无对应项。
//! - `writable_tmp_mb`：`RLIMIT_FSIZE` 限制单文件大小，不是临时目录总量。
//! - `wall_time_seconds` / `output_bytes`：已由执行器自身的超时与输出采集预算强制。

// rlimit 常量在不同平台上类型不同（macOS 是 c_int，Linux 的 __rlimit_resource_t 是 u32）。
// 这里统一按 i32 传递再按需转换，所以 macOS 上会被 clippy 判为多余转换——不能按单一平台删掉。
#![allow(clippy::unnecessary_cast)]

use crate::agent::sandbox::ResourceLimits;

/// 本次要施加到子进程的 rlimit（None = 不限制）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeLimits {
    pub cpu_seconds: Option<u64>,
    pub address_space_bytes: Option<u64>,
}

/// 实际施加结果：`applied` 是内核接受的限额，`skipped` 是未施加项及原因。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeLimitsReport {
    pub applied: Vec<(&'static str, u64)>,
    pub skipped: Vec<(&'static str, String)>,
}

impl NativeLimitsReport {
    pub fn is_empty(&self) -> bool {
        self.applied.is_empty() && self.skipped.is_empty()
    }

    /// 供事件/日志展示的一行摘要；未施加项必须能被看见，不能只报告成功项。
    pub fn summary(&self) -> String {
        let applied = self
            .applied
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(", ");
        let skipped = self
            .skipped
            .iter()
            .map(|(name, reason)| format!("{name}(未施加: {reason})"))
            .collect::<Vec<_>>()
            .join(", ");
        match (applied.is_empty(), skipped.is_empty()) {
            (true, true) => "未请求资源限制".into(),
            (false, true) => format!("已施加 {applied}"),
            (true, false) => format!("未施加任何限制：{skipped}"),
            (false, false) => format!("已施加 {applied}；{skipped}"),
        }
    }
}

/// 宿主直跑（默认兼容模式）的限额环境变量。未设置即不限制——直跑是显式兼容模式，
/// 默认给每条命令套 CPU/内存上限会打断正常构建，因此这里必须由使用者显式开启。
pub const HOST_DIRECT_CPU_SECONDS_ENV: &str = "HARMONY_HOST_DIRECT_CPU_SECONDS";
pub const HOST_DIRECT_MEMORY_MB_ENV: &str = "HARMONY_HOST_DIRECT_MEMORY_MB";

/// 解析宿主直跑限额配置；变量存在但取值非法时失败关闭（与 sandbox 配置同口径），
/// 避免把写错的限额当成「没配置」而静默不限制。
pub fn parse_host_direct_limits(
    cpu_seconds: Option<&str>,
    memory_mb: Option<&str>,
) -> Result<NativeLimits, String> {
    let parse = |name: &str, raw: Option<&str>, min: u64, max: u64| -> Result<Option<u64>, String> {
        let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let value: u64 = raw
            .parse()
            .map_err(|_| format!("{name}={raw} 不是合法整数；留空表示不限制"))?;
        if !(min..=max).contains(&value) {
            return Err(format!("{name}={value} 超出允许范围 {min}..={max}"));
        }
        Ok(Some(value))
    };
    Ok(NativeLimits {
        cpu_seconds: parse(HOST_DIRECT_CPU_SECONDS_ENV, cpu_seconds, 1, 3_600)?,
        address_space_bytes: parse(HOST_DIRECT_MEMORY_MB_ENV, memory_mb, 64, 65_536)?
            .map(|mb| mb * 1024 * 1024),
    })
}

/// 读取宿主直跑限额配置（未配置 = 不限制）。
pub fn host_direct_limits_from_env() -> Result<NativeLimits, String> {
    let cpu = std::env::var(HOST_DIRECT_CPU_SECONDS_ENV).ok();
    let memory = std::env::var(HOST_DIRECT_MEMORY_MB_ENV).ok();
    parse_host_direct_limits(cpu.as_deref(), memory.as_deref())
}

/// 从沙箱 spec 推导原生可表达的限额（其余限额见模块头说明）。
pub fn from_spec(limits: &ResourceLimits) -> NativeLimits {
    NativeLimits {
        cpu_seconds: Some(limits.cpu_seconds),
        address_space_bytes: Some(limits.memory_mb.saturating_mul(1024 * 1024)),
    }
}

/// 平台能力说明：这些限额在当前平台无法用 rlimit 表达，调用方必须原样展示。
pub fn platform_gaps() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "pids",
            "RLIMIT_NPROC 按用户全局计数，与容器内进程数语义不同，不映射",
        ),
        ("cpu_count", "CPU 配额需要 cgroups，rlimit 无对应项"),
        (
            "writable_tmp_mb",
            "RLIMIT_FSIZE 限制单文件大小而非临时目录总量",
        ),
    ]
}

#[cfg(unix)]
fn probe(resource: i32) -> bool {
    unsafe {
        let mut current = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(resource as _, &mut current) != 0 {
            return false;
        }
        // 用当前值回写是语义空操作：成功即说明内核接受对该限额的修改。
        // 若改为「尝试设一个新值」来探测，会真的改掉本进程的限额。
        libc::setrlimit(resource as _, &current) == 0
    }
}

/// 当前平台是否接受修改地址空间限额（macOS 实测为否）。
pub fn address_space_supported() -> bool {
    #[cfg(unix)]
    {
        static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *SUPPORTED.get_or_init(|| probe(libc::RLIMIT_AS as i32))
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// 当前平台是否接受修改 CPU 时间限额。
pub fn cpu_time_supported() -> bool {
    #[cfg(unix)]
    {
        static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *SUPPORTED.get_or_init(|| probe(libc::RLIMIT_CPU as i32))
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// 把限额挂到子进程上（fork 之后、exec 之前生效），返回实际施加情况。
///
/// 不支持的限额不会阻断命令：它们只是不生效，并由 `skipped` 如实报告——
/// 调用方据此判断「这次执行并没有被该项限制保护」，而不是误以为已经限制。
pub fn apply(cmd: &mut tokio::process::Command, limits: &NativeLimits) -> NativeLimitsReport {
    let mut report = NativeLimitsReport::default();
    let requested = [
        (limits.cpu_seconds, UNSUPPORTED_CPU_REASON, "cpu_seconds"),
        (
            limits.address_space_bytes,
            UNSUPPORTED_ADDRESS_SPACE_REASON,
            "address_space",
        ),
    ];
    for (value, unsupported, name) in requested {
        let Some(value) = value else { continue };
        let supported = if name == "cpu_seconds" {
            cpu_time_supported()
        } else {
            address_space_supported()
        };
        if !supported {
            report.skipped.push((name, unsupported.to_string()));
            continue;
        }
        report.applied.push((name, value));
        attach_rlimit(cmd, name, value);
    }
    report
}

/// 非 unix 平台没有 rlimit：请求了就如实报告不可用，不做任何伪装。
#[cfg(not(unix))]
const UNSUPPORTED_CPU_REASON: &str = "当前平台不支持 POSIX rlimit（CPU 时间限额）";
#[cfg(not(unix))]
const UNSUPPORTED_ADDRESS_SPACE_REASON: &str = "当前平台不支持 POSIX rlimit（地址空间限额）";
/// unix 上是否支持由运行时探测决定，这里的文案只在探测失败时使用。
#[cfg(unix)]
const UNSUPPORTED_CPU_REASON: &str = "当前平台内核不接受 CPU 时间限额（setrlimit 探测失败）";
#[cfg(unix)]
const UNSUPPORTED_ADDRESS_SPACE_REASON: &str =
    "当前平台内核不接受地址空间限额（如 macOS 的 RLIMIT_AS 返回 EINVAL）";

/// 在 fork 之后、exec 之前设置 rlimit。非 unix 平台是空实现（限额已在报告里标为未施加）。
#[cfg(unix)]
fn attach_rlimit(cmd: &mut tokio::process::Command, name: &'static str, value: u64) {
    let which = if name == "cpu_seconds" {
        libc::RLIMIT_CPU as i32
    } else {
        libc::RLIMIT_AS as i32
    };
    unsafe {
        cmd.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: value,
                rlim_max: value,
            };
            if libc::setrlimit(which as _, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn attach_rlimit(_cmd: &mut tokio::process::Command, _name: &'static str, _value: u64) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_limits(cpu_seconds: u64, memory_mb: u64) -> ResourceLimits {
        ResourceLimits {
            cpu_seconds,
            memory_mb,
            ..ResourceLimits::default()
        }
    }

    #[test]
    fn maps_only_semantically_equivalent_limits() {
        let limits = from_spec(&spec_limits(30, 512));
        assert_eq!(limits.cpu_seconds, Some(30));
        assert_eq!(limits.address_space_bytes, Some(512 * 1024 * 1024));
        // 进程数/CPU 配额/临时目录总量不映射：原因必须可展示
        let gaps = platform_gaps();
        assert!(gaps.iter().any(|(name, _)| *name == "pids"));
        assert!(gaps.iter().all(|(_, reason)| !reason.is_empty()));
    }

    /// 宿主直跑限额是显式配置：未设置 = 不限制，写错 = 失败关闭而不是静默忽略。
    #[test]
    fn host_direct_limits_are_opt_in_and_fail_closed_on_typos() {
        assert_eq!(
            parse_host_direct_limits(None, None).unwrap(),
            NativeLimits::default()
        );
        assert_eq!(
            parse_host_direct_limits(Some(""), Some("  ")).unwrap(),
            NativeLimits::default()
        );
        let parsed = parse_host_direct_limits(Some("120"), Some("2048")).unwrap();
        assert_eq!(parsed.cpu_seconds, Some(120));
        assert_eq!(parsed.address_space_bytes, Some(2048 * 1024 * 1024));
        for (cpu, memory) in [
            (Some("abc"), None),
            (Some("0"), None),
            (Some("99999"), None),
            (None, Some("12")),
            (None, Some("0")),
        ] {
            assert!(
                parse_host_direct_limits(cpu, memory).is_err(),
                "cpu={cpu:?} memory={memory:?} 应被拒绝"
            );
        }
    }

    #[test]
    fn report_never_hides_an_unapplied_limit() {
        let report = NativeLimitsReport {
            applied: vec![("cpu_seconds", 30)],
            skipped: vec![("address_space", UNSUPPORTED_ADDRESS_SPACE_REASON.to_string())],
        };
        let summary = report.summary();
        assert!(summary.contains("cpu_seconds=30"), "{summary}");
        assert!(summary.contains("address_space(未施加"), "{summary}");
    }

    /// 真实子进程：CPU 时间限额必须由内核执行。
    #[tokio::test]
    async fn cpu_limit_terminates_a_spinning_child() {
        if !cpu_time_supported() {
            eprintln!("跳过：当前平台不接受 CPU 时间限额");
            return;
        }
        let mut cmd = crate::utils::process::command(
            "/bin/sh",
            &[
                "-c".to_string(),
                "x=0; while :; do x=$((x+1)); done".to_string(),
            ],
        )
        .expect("sh 可用");
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let report = apply(
            &mut cmd,
            &NativeLimits {
                cpu_seconds: Some(1),
                address_space_bytes: None,
            },
        );
        assert_eq!(report.applied, vec![("cpu_seconds", 1)]);
        let started = std::time::Instant::now();
        let output = cmd.output().await.expect("子进程应能启动");
        // 被信号终止时没有退出码：这是「内核真的执行了限额」的证据
        assert!(!output.status.success(), "自旋进程不应正常退出");
        assert!(
            output.status.code().is_none(),
            "限额生效时应由信号终止，实际 code={:?}",
            output.status.code()
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "应在 CPU 时间耗尽后很快终止"
        );
    }

    /// 地址空间限额：只在平台接受时声明已应用，否则如实进 skipped。
    #[tokio::test]
    async fn memory_limit_is_reported_according_to_platform_support() {
        let mut cmd = crate::utils::process::command("/bin/echo", &[]).expect("echo 可用");
        let report = apply(
            &mut cmd,
            &NativeLimits {
                cpu_seconds: None,
                address_space_bytes: Some(64 * 1024 * 1024),
            },
        );
        if address_space_supported() {
            assert_eq!(report.applied, vec![("address_space", 64 * 1024 * 1024)]);
            assert!(report.skipped.is_empty());
        } else {
            assert!(report.applied.is_empty());
            assert_eq!(report.skipped.len(), 1);
            assert!(!report.skipped[0].1.is_empty(), "未施加必须给出原因");
        }
    }

    /// 真实调用执行器入口：宿主直跑路径配置了 CPU 限额后，子进程必须被内核终止，
    /// 且报告里如实标注已施加——这条覆盖的是「配置 → 执行器 → 子进程」整条接线。
    #[tokio::test]
    async fn streaming_runner_enforces_configured_host_limits() {
        if !cpu_time_supported() {
            eprintln!("跳过：当前平台不接受 CPU 时间限额");
            return;
        }
        let limits = parse_host_direct_limits(Some("1"), None).unwrap();
        let ctx = crate::agent::exec_ctx::ToolCtx::empty();
        let started = std::time::Instant::now();
        let (output, _truncated, report) =
            crate::agent::exec_ctx::run_cmd_streaming_env_with_native_limits(
                &ctx,
                "/bin/sh",
                &[
                    "-c".to_string(),
                    "x=0; while :; do x=$((x+1)); done".to_string(),
                ],
                None,
                60,
                None,
                &limits,
            )
            .await
            .expect("执行器应能启动命令");
        assert_eq!(report.applied, vec![("cpu_seconds", 1)]);
        assert!(!output.status.success(), "自旋命令不应正常退出");
        assert!(output.status.code().is_none(), "限额生效时应由信号终止");
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
    }

    #[tokio::test]
    async fn no_limits_means_no_report_and_no_wrapping() {
        let mut cmd =
            crate::utils::process::command("/bin/echo", &["ok".to_string()]).expect("echo 可用");
        let report = apply(&mut cmd, &NativeLimits::default());
        assert!(report.is_empty());
        let output = cmd.output().await.expect("命令仍应正常执行");
        assert!(String::from_utf8_lossy(&output.stdout).contains("ok"));
    }
}
