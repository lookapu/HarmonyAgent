//! MCP 连接管理器：全局 AppState，按服务器缓存长驻子进程客户端。
//! - 惰性连接：首次用到（拉取工具清单 / 调用工具）时启动，之后复用；
//! - 进程异常退出：call 失败时移除缓存，下次调用自动重新拉起；
//! - 失败退避：连接失败后指数冷却并自动重试，避免瞬时故障永久禁用，也防反复拉起；
//! - 并发单飞：同一服务器首连只启动一个子进程，其余调用等待并复用结果；
//! - 应用退出：shutdown_all 统一终止全部子进程（含孙进程）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::mcp_client::{McpClient, McpToolDef};
use crate::db::models::McpServer;

#[derive(Default)]
pub struct McpManager {
    /// 服务器 id → 长驻连接
    clients: StdMutex<HashMap<String, Arc<McpClient>>>,
    /// 连接失败状态：短期冷却后自动重试，不因一次临时故障永久禁用整个应用生命周期。
    failed: StdMutex<HashMap<String, FailureState>>,
    /// 每个实例独立的首连单飞门控，避免并发工具清单请求重复拉起同一个 MCP 子进程。
    connect_gates: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Clone)]
struct FailureState {
    attempts: u32,
    failed_at: Instant,
    last_error: String,
}

fn failure_cooldown(attempts: u32) -> Duration {
    Duration::from_secs(5_u64.saturating_mul(1_u64 << attempts.saturating_sub(1).min(4)))
}

impl McpManager {
    /// 获取或建立服务器连接（已缓存且存活则复用）。
    /// 并发首连时后到者丢弃自己新建的连接（Drop 自动终止子进程），保证缓存唯一。
    /// 连接失败进入有界指数冷却，冷却结束后自动重试。
    pub async fn get_or_connect(
        &self,
        server: &McpServer,
        project_root: &std::path::Path,
    ) -> Result<Arc<McpClient>, String> {
        let project_id = server
            .project_id
            .as_deref()
            .ok_or("全局 MCP 配置不能直接进入 Agent；请先克隆到项目并授权")?;
        crate::services::mcp_policy::ensure_server_authorized(server, project_id)?;
        if let Some(c) = self.clients.lock().unwrap_or_else(|e| e.into_inner()).get(&server.id) {
            return Ok(c.clone());
        }
        let gate = self
            .connect_gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(server.id.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _connect_guard = gate.lock().await;
        // 等待单飞门控期间，前一个调用可能已经完成连接。
        if let Some(c) = self.clients.lock().unwrap_or_else(|e| e.into_inner()).get(&server.id) {
            return Ok(c.clone());
        }
        if let Some(failure) = self.failed.lock().unwrap_or_else(|e| e.into_inner()).get(&server.id).cloned() {
            let cooldown = failure_cooldown(failure.attempts);
            if let Some(remaining) = cooldown.checked_sub(failure.failed_at.elapsed()) {
                return Err(format!(
                    "MCP 连接冷却中（{}s 后自动重试，已失败 {} 次）：{}",
                    remaining.as_secs().saturating_add(1),
                    failure.attempts,
                    failure.last_error
                ));
            }
        }
        let client = match McpClient::connect(server, project_root).await {
            Ok(c) => c,
            Err(e) => {
                self.record_failure(&server.id, &e);
                return Err(e);
            }
        };
        let client = Arc::new(client);
        self.failed.lock().unwrap_or_else(|e| e.into_inner()).remove(&server.id);
        let mut map = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        map.insert(server.id.clone(), client.clone());
        Ok(client)
    }

    fn record_failure(&self, server_id: &str, error: &str) {
        let mut failed = self.failed.lock().unwrap_or_else(|e| e.into_inner());
        let attempts = failed.get(server_id).map_or(1, |f| f.attempts.saturating_add(1));
        failed.insert(
            server_id.to_string(),
            FailureState {
                attempts,
                failed_at: Instant::now(),
                last_error: error.chars().take(300).collect(),
            },
        );
    }

    /// 连接测试成功：清除失败标记（用户修复配置后恢复参与工具注入）
    pub fn mark_connected(&self, server_id: &str) {
        self.failed.lock().unwrap_or_else(|e| e.into_inner()).remove(server_id);
    }

    /// 按服务器 id 精确调用。调用方必须先从当前项目授权查询解析实例并复验策略；
    /// 管理器不再提供按名称或全局回退路由，避免未来调用绕开项目绑定。
    pub async fn call_by_id(&self, server_id: &str, tool: &str, args: Value) -> Result<String, String> {
        let client = self.clients.lock().unwrap_or_else(|e| e.into_inner()).get(server_id).cloned();
        let Some(client) = client else {
            return Err(format!("MCP 服务器实例（{server_id}）未连接，请重试"));
        };
        match client.call_tool(tool, args).await {
            Ok(out) => Ok(out),
            Err(e) => {
                self.clients.lock().unwrap_or_else(|e| e.into_inner()).remove(&client.server_id);
                self.record_failure(&client.server_id, &e);
                Err(e)
            }
        }
    }

    /// 拉取已启用服务器的工具清单（逐台独立失败，互不影响；并行连接，总耗时≈最慢单台）
    pub async fn collect_tools(
        &self,
        servers: &[McpServer],
        project_root: &std::path::Path,
    ) -> Vec<(String, Vec<McpToolDef>, Result<(), String>)> {
        let futs: Vec<_> = servers
            .iter()
            .map(|server| async move {
                let name = server.name.clone();
                match self.get_or_connect(server, project_root).await {
                    Ok(c) => match c.list_tools().await {
                        Ok(tools) => (name, tools, Ok(())),
                        Err(e) => {
                            self.clients.lock().unwrap_or_else(|p| p.into_inner()).remove(&c.server_id);
                            self.record_failure(&c.server_id, &e);
                            (name, Vec::new(), Err(e))
                        }
                    },
                    Err(e) => (name, Vec::new(), Err(e)),
                }
            })
            .collect();
        futures_util::future::join_all(futs).await
    }

    /// 终止全部 MCP 子进程（应用退出时调用；缓存清空，下次启动重新连接）
    pub fn shutdown_all(&self) {
        let clients: Vec<Arc<McpClient>> =
            self.clients.lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, c)| c).collect();
        drop(clients); // Arc 释放触发 Drop → 进程树强杀
    }

    /// 断开指定服务器的连接（配置删除/项目删除时调用，避免子进程残留到退出）
    pub fn disconnect(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        {
            let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
            for id in ids {
                clients.remove(id); // Arc 释放触发 Drop → 进程树强杀
            }
        }
        let mut failed = self.failed.lock().unwrap_or_else(|e| e.into_inner());
        for id in ids {
            failed.remove(id);
        }
        let mut gates = self.connect_gates.lock().unwrap_or_else(|e| e.into_inner());
        for id in ids {
            gates.remove(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_cooldown_is_bounded_exponential_backoff() {
        assert_eq!(failure_cooldown(1), Duration::from_secs(5));
        assert_eq!(failure_cooldown(2), Duration::from_secs(10));
        assert_eq!(failure_cooldown(5), Duration::from_secs(80));
        assert_eq!(failure_cooldown(99), Duration::from_secs(80));
    }

    #[test]
    fn mark_connected_recovers_failed_instance() {
        let manager = McpManager::default();
        manager.record_failure("server", "temporary");
        assert!(manager.failed.lock().unwrap().contains_key("server"));
        manager.mark_connected("server");
        assert!(!manager.failed.lock().unwrap().contains_key("server"));
    }
}
