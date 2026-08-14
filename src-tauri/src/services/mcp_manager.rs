//! MCP 连接管理器：全局 AppState，按服务器缓存长驻子进程客户端。
//! - 惰性连接：首次用到（拉取工具清单 / 调用工具）时启动，之后复用；
//! - 进程异常退出：call 失败时移除缓存，下次调用自动重新拉起；
//! - 失败标记：连接失败的服务器在本次运行期间跳过，避免每次对话反复尝试拖慢主流程；
//! - 应用退出：shutdown_all 统一终止全部子进程（含孙进程）。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::Value;

use super::mcp_client::{McpClient, McpToolDef};
use crate::db::models::McpServer;

#[derive(Default)]
pub struct McpManager {
    /// 服务器 id → 长驻连接
    clients: StdMutex<HashMap<String, Arc<McpClient>>>,
    /// 本次运行期间连接失败的服务器 id（测试连接成功后清除）
    failed: StdMutex<HashSet<String>>,
}

impl McpManager {
    /// 获取或建立服务器连接（已缓存且存活则复用）。
    /// 并发首连时后到者丢弃自己新建的连接（Drop 自动终止子进程），保证缓存唯一。
    /// 连接失败会记入失败标记，本次运行期间不再重试（见 struct 注释）。
    pub async fn get_or_connect(&self, server: &McpServer) -> Result<Arc<McpClient>, String> {
        if let Some(c) = self.clients.lock().unwrap().get(&server.id) {
            return Ok(c.clone());
        }
        if self.failed.lock().unwrap().contains(&server.id) {
            return Err("连接已标记失败（本次运行跳过，可在 MCP 页重新测试连接恢复）".into());
        }
        let client = match McpClient::connect(server).await {
            Ok(c) => c,
            Err(e) => {
                self.failed.lock().unwrap().insert(server.id.clone());
                return Err(e);
            }
        };
        let client = Arc::new(client);
        let mut map = self.clients.lock().unwrap();
        Ok(map.entry(server.id.clone()).or_insert(client).clone())
    }

    /// 连接测试成功：清除失败标记（用户修复配置后恢复参与工具注入）
    pub fn mark_connected(&self, server_id: &str) {
        self.failed.lock().unwrap().remove(server_id);
    }

    /// 调用 MCP 工具（工具名格式 mcp__服务器名__工具名）。
    /// 同名服务器（全局 + 项目级）路由：优先同项目连接，其次全局连接；绝不跨项目路由。
    /// 进程异常退出时移除缓存连接并报错（下次调用自动重连）。
    pub async fn call(
        &self,
        server_name: &str,
        tool: &str,
        args: Value,
        project_id: Option<&str>,
    ) -> Result<String, String> {
        // 按服务器名查找（连接缓存以 id 为 key，名称仅作展示层匹配）
        let client = {
            let map = self.clients.lock().unwrap();
            map.values()
                .filter(|c| c.server_name == server_name)
                .find(|c| c.project_id.as_deref() == project_id)
                .or_else(|| {
                    map.values()
                        .filter(|c| c.server_name == server_name && c.project_id.is_none())
                        .next()
                })
                .cloned()
        };
        let Some(client) = client else {
            return Err(format!("MCP 服务器「{server_name}」未连接（可能启动失败或被移除），请重试"));
        };
        match client.call_tool(tool, args).await {
            Ok(out) => Ok(out),
            Err(e) => {
                // 进程级故障（退出/超时）移除缓存，允许下次调用重新拉起
                self.clients.lock().unwrap().remove(&client.server_id);
                Err(e)
            }
        }
    }

    /// 按服务器 id 精确调用（同名多实例路由：name#n 已由调用方解析为具体实例 id）
    pub async fn call_by_id(&self, server_id: &str, tool: &str, args: Value) -> Result<String, String> {
        let client = self.clients.lock().unwrap().get(server_id).cloned();
        let Some(client) = client else {
            return Err(format!("MCP 服务器实例（{server_id}）未连接，请重试"));
        };
        match client.call_tool(tool, args).await {
            Ok(out) => Ok(out),
            Err(e) => {
                self.clients.lock().unwrap().remove(&client.server_id);
                Err(e)
            }
        }
    }

    /// 拉取已启用服务器的工具清单（逐台独立失败，互不影响；并行连接，总耗时≈最慢单台）
    pub async fn collect_tools(
        &self,
        servers: &[McpServer],
    ) -> Vec<(String, Vec<McpToolDef>, Result<(), String>)> {
        let futs: Vec<_> = servers
            .iter()
            .map(|server| async move {
                let name = server.name.clone();
                match self.get_or_connect(server).await {
                    Ok(c) => match c.list_tools().await {
                        Ok(tools) => (name, tools, Ok(())),
                        Err(e) => (name, Vec::new(), Err(e)),
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
            self.clients.lock().unwrap().drain().map(|(_, c)| c).collect();
        drop(clients); // Arc 释放触发 Drop → 进程树强杀
    }

    /// 断开指定服务器的连接（配置删除/项目删除时调用，避免子进程残留到退出）
    pub fn disconnect(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        {
            let mut clients = self.clients.lock().unwrap();
            for id in ids {
                clients.remove(id); // Arc 释放触发 Drop → 进程树强杀
            }
        }
        let mut failed = self.failed.lock().unwrap();
        for id in ids {
            failed.remove(id);
        }
    }
}
