pub mod agent;
// pub：tauri 命令宏要求命令返回类型（如 chat::TaskLedger）在 crate 外可见
pub mod commands;
pub mod db;
pub mod services;
mod tray;
mod utils;

use tauri::{Emitter, Manager, RunEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use commands::proxy::{ProxyLock, ProxyState};
use commands::chat::{ChatCancel, ChatLock, ToolApprovalState};
use utils::task_registry::TaskRegistry;
use commands::lan::LanServerState;
use services::proxy_service::{ProxyConfig, ProxyServer};

/// 随应用启动自动开启本地代理（proxy_config.enabled = 1 时）
fn auto_start_proxy(app: &tauri::AppHandle, db_path: &std::path::Path) {
    // 读取自动启动开关
    let enabled = match rusqlite::Connection::open(db_path) {
        Ok(conn) => conn
            .query_row("SELECT enabled FROM proxy_config WHERE id = 1", [], |row| {
                row.get::<_, i32>(0)
            })
            .unwrap_or(0)
            != 0,
        Err(_) => false,
    };
    if !enabled {
        return;
    }

    // 多开保护：仅代理锁持有者自动启动（其余实例共享其代理，不重复启动）
    let is_owner = app
        .try_state::<ProxyLock>()
        .map(|l| l.0.try_lock().map(|g| g.is_some()).unwrap_or(false))
        .unwrap_or(false);
    if !is_owner {
        eprintln!("[proxy] 已在其他实例中运行，跳过自动启动");
        return;
    }

    // 代理运行期间长期持有独立连接（不与池内连接互相阻塞）
    let proxy_conn = match rusqlite::Connection::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[proxy] auto-start: open db failed: {}", e);
            return;
        }
    };
    if let Err(e) = proxy_conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;") {
        eprintln!("[proxy] auto-start: pragma failed: {}", e);
        return;
    }
    let proxy_db = std::sync::Arc::new(std::sync::Mutex::new(proxy_conn));

    let config = match rusqlite::Connection::open(db_path) {
        Ok(conn) => conn
            .query_row(
                "SELECT listen_address, listen_port, auto_failover, max_retries,
                        streaming_first_byte_timeout_s, non_streaming_timeout_s
                 FROM proxy_config WHERE id = 1",
                [],
                |row| {
                    Ok(ProxyConfig {
                        listen_address: row.get(0)?,
                        listen_port: row.get::<_, i32>(1)? as u16,
                        auto_failover: row.get::<_, i32>(2)? != 0,
                        max_retries: row.get::<_, i32>(3)? as u32,
                        streaming_first_byte_timeout_s: row.get::<_, i32>(4)? as u64,
                        non_streaming_timeout_s: row.get::<_, i32>(5)? as u64,
                    })
                },
            )
            .unwrap_or_default(),
        Err(_) => ProxyConfig::default(),
    };

    let proxy_state = app.state::<ProxyState>();
    tauri::async_runtime::block_on(async move {
        let mut server = proxy_state.0.lock().await;
        if let Err(e) = server.start(proxy_db, config).await {
            eprintln!("[proxy] auto-start failed: {}", e);
        }
    });
}

/// 随应用启动自动开启局域网访问服务（lan_config.enabled = 1 时）
fn auto_start_lan(app: &tauri::AppHandle, db_path: &std::path::Path) {
    // 读取自动启动开关
    let enabled = match rusqlite::Connection::open(db_path) {
        Ok(conn) => conn
            .query_row("SELECT enabled FROM lan_config WHERE id = 1", [], |row| {
                row.get::<_, i32>(0)
            })
            .unwrap_or(0)
            != 0,
        Err(_) => false,
    };
    if !enabled {
        return;
    }

    let app2 = app.clone();
    tauri::async_runtime::block_on(async move {
        let db = app2.state::<crate::db::DbState>();
        let state = app2.state::<LanServerState>();
        // clone 传入所有权参数，State 借用 app2 不受影响
        if let Err(e) = commands::lan::start_lan_server(app2.clone(), db, state).await {
            eprintln!("[lan] auto-start failed: {}", e);
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_os::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let key = shortcut.key;
                    // Ctrl+Alt+Space：唤起/隐藏主窗口
                    if key == Code::Space {
                        if let Some(win) = app.get_webview_window("main") {
                            let visible = win.is_visible().unwrap_or(false);
                            if visible && win.is_focused().unwrap_or(false) {
                                let _ = win.hide();
                            } else {
                                commands::desktop::focus_main_window(app);
                            }
                        } else {
                            commands::desktop::focus_main_window(app);
                        }
                    } else if key == Code::KeyN {
                        // Ctrl+Alt+N：新建对话
                        commands::desktop::focus_main_window(app);
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.emit("tray-new-chat", ());
                        }
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // 注册工具执行流水线护栏钩子（预算/黑名单/审批/进度/大输出落盘），幂等
            crate::agent::tools::guards::ensure_registered();
            let app_handle = app.handle().clone();
            let db_path = app_handle
                .path()
                .app_data_dir()
                .expect("failed to resolve app data dir")
                .join("deveco-switch.db");

            std::fs::create_dir_all(db_path.parent().unwrap()).ok();

            // 任务级 JSON 日志（排障用；失败静默，不影响主流程）
            crate::utils::logger::init(
                app_handle.path().app_data_dir().unwrap().join("logs"),
            );

            // 符号索引磁盘缓存目录：重启后命中，只对变化文件增量重扫
            crate::services::symbol_index::init_cache_dir(
                app_handle
                    .path()
                    .app_data_dir()
                    .unwrap_or_default()
                    .join("symbol_cache"),
            );

            let pool = db::init(&db_path).expect("failed to initialize database");
            let pool_arc = std::sync::Arc::new(pool);

            // 注册全局 DB 单例（todo 持久化等无 State 上下文的模块使用同一连接）
            db::register_global(std::sync::Arc::clone(&pool_arc));

            // Agent 护栏参数动态配置：从 settings 表加载用户设置（无配置时用默认值）
            services::agent_limits::init_from_db();

            // 启动时数据维护：按保留策略滚动清理日志/成本明细（不阻塞启动）
            if let Ok(conn) = pool_arc.lock() {
                services::maintenance::run_startup_maintenance(&conn);
            }

            // Execution Kernel Worker 心跳：每个桌面进程都有唯一实例身份。恢复扫描只回收
            // 真正过期/失联的所有者，因此第二个实例启动不会中断第一个实例的任务。
            {
                let worker_db = std::sync::Arc::clone(&pool_arc);
                tauri::async_runtime::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
                    loop {
                        ticker.tick().await;
                        let Ok(conn) = worker_db.try_lock() else { continue };
                        let _ = crate::agent::scheduler::heartbeat_worker(
                            &conn,
                            crate::agent::scheduler::current_worker_id(),
                        );
                        let _ = crate::agent::scheduler::recover_stale_owners(&conn, 60_000);
                    }
                });
            }

            // 内置 API 知识库种子数据：主库三张 API 表为空（从未抓取过）时，
            // 后台线程从资源目录 seed/knowledge.db 导入（新装用户开箱即用，
            // 无需联网抓取；导入失败静默，不影响启动）
            services::seed::seed_api_knowledge(
                &db_path,
                app_handle.path().resource_dir().ok(),
            );

            // ohpm 三方库推荐缓存：超过 7 天未更新时启动后后台静默刷新
            // （一次 GET 全量替换，秒级；失败静默，不影响启动）
            {
                let db_arc = std::sync::Arc::clone(&pool_arc);
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                    let need = {
                        let Ok(conn) = db_arc.lock() else { return };
                        services::ohpm_landscape::needs_refresh(&conn, 7)
                    };
                    if need {
                        let _ = services::ohpm_landscape::refresh(&db_arc).await;
                    }
                });
            }

            // 会话内定时提醒轮询：每 30 秒派发到期提醒到对应会话队列
            // （对齐 deepseek-harness schedule：session-local 投递，不中断当前轮次，
            //   注入后模型下一轮请求自动看到；应用退出即不再投递）
            {
                let db_arc = std::sync::Arc::clone(&pool_arc);
                let app2 = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
                    loop {
                        ticker.tick().await;
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        let due = {
                            let Ok(conn) = db_arc.lock() else { continue };
                            match crate::services::reminders::dispatch_due(&conn, now) {
                                Ok(d) => d,
                                Err(_) => continue,
                            }
                        };
                        for (conv_id, prompt) in due {
                            crate::agent::session_ctx::inject_message(&conv_id, prompt.clone());
                            crate::commands::desktop::send_notification(
                                app2.clone(),
                                "定时提醒".to_string(),
                                prompt,
                                Some("reminder".to_string()),
                            );
                        }
                    }
                });
            }

            // 注入资源目录给 embedding 模块（打包后语义模型在 resource_dir/embedding/…）
            #[cfg(feature = "embedding")]
            services::embedding::set_resource_dir(
                app_handle.path().resource_dir().unwrap_or_default(),
            );

            // 确保全局项目存在（未添加任何项目时的默认工作区）；
            // 默认工作目录：用户主目录下的 DevecoSwitch 子目录（启动时自动创建），
            // 仅在 path 为空时写入，不覆盖用户后续自定义的路径
            if let Ok(conn) = pool_arc.lock() {
                conn.execute(
                    "INSERT OR IGNORE INTO projects
                        (id, name, path, kind, trusted, index_state, created_at, last_opened_at)
                     VALUES ('global', '全局项目', '', 'global', 1, 'ready', 0, 0)",
                    [],
                )
                .ok();
                let default_ws = std::env::var_os("USERPROFILE")
                    .or_else(|| std::env::var_os("HOME"))
                    .map(std::path::PathBuf::from)
                    .map(|h| h.join("DevecoSwitch"));
                if let Some(ws) = default_ws {
                    // 目录创建失败不阻塞启动（path 仍写入，文件工具会报目录不存在）
                    std::fs::create_dir_all(&ws).ok();
                    conn.execute(
                        "UPDATE projects SET path = ?1 WHERE id = 'global' AND (path IS NULL OR path = '')",
                        [&ws.to_string_lossy().to_string()],
                    )
                    .ok();
                }
            }

            app.manage(db::DbState(pool_arc));

            app.manage(ProxyState(tokio::sync::Mutex::new(ProxyServer::new())));
            app.manage(LanServerState(tokio::sync::Mutex::new(
                services::lan_server::LanServer::new(),
            )));
            app.manage(ChatLock::default());
            app.manage(ChatCancel::default());
            app.manage(ToolApprovalState::default());
            app.manage(commands::chat::SessionToolAllowState::default());
            app.manage(commands::chat::FirstWriteApprovedState::default());
            app.manage(commands::chat::DiagnoseCardState::default());
            app.manage(commands::chat::PlanApprovalState::default());
            // 任务监管注册表：心跳 + 看门狗强杀（卡死/停止失效兜底）
            app.manage(TaskRegistry::default());
            // MCP 连接管理器：按服务器缓存长驻子进程客户端（惰性连接，退出时统一清理）
            app.manage(services::mcp_manager::McpManager::default());

            // 代理互斥锁：多开时仅首个实例持有（负责启动/停止本地代理）
            let lock_data_dir = app_handle.path().app_data_dir().unwrap_or_default();
            app.manage(ProxyLock(tokio::sync::Mutex::new(
                commands::proxy::acquire_proxy_lock(&lock_data_dir),
            )));
            tray::setup(&app_handle)?;

            // 全局快捷键：Ctrl+Alt+Space 唤起/隐藏窗口，Ctrl+Alt+N 新建对话
            let show_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space);
            let new_chat_shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyN);
            let gsm = app_handle.global_shortcut();
            let _ = gsm.register(show_shortcut);
            let _ = gsm.register(new_chat_shortcut);

            // 注册内置 Node 运行时目录（系统 PATH 未命中 npx 时的兑底）
            services::node_runtime::init_node_runtime(&app_handle);

            // 注册内置 Git 运行时目录（系统未装 Git 时分支工作流/文档下载兑底）
            services::git_runtime::init_git_runtime(&app_handle);

            // 隐藏控制台：使 hvigor worker 等孙进程继承隐藏控制台而非新建窗口（防弹 cmd）
            crate::utils::process::init_hidden_console();

            // 注册内置 JDK 运行时（多版本，默认版本目录；系统无 JDK 时自动注入 JAVA_HOME，
            // 使 hvigor 构建在无 DevEco JBR/系统 Java 的机器上也能工作）
            services::jdk_runtime::init_jdk_runtime(&app_handle);

            // 鸿蒙环境探测：自动发现 SDK / command-line-tools，把 hdc/ohpm 所在目录
            // 注入子进程 PATH，使设备/构建工具在未配置系统 PATH 时也能正常调用
            {
                let db_state = app_handle.state::<db::DbState>();
                // 软件内置工具链目录（app_data/toolkits/command-line-tools）作为 cli 候选
                let data_dir = app_handle.path().app_data_dir().ok();
                services::harmony_env::set_bundled_cli_dir(
                    data_dir.as_ref().map(|d| d.join("toolkits").join("command-line-tools")),
                );
                let env = services::harmony_env::detect(&db_state);
                crate::utils::process::set_harmony_path_dirs(
                    services::harmony_env::path_dirs(&env),
                );
            }

            // 注册 App 专属 npm 缓存根目录：MCP 子进程 npx 按服务器隔离缓存，
            // 避免多进程并发写系统全局 npm 缓存时 Windows 文件锁冲突（EPERM）
            crate::utils::process::set_mcp_npm_cache_root(
                app_handle.path().app_data_dir().unwrap().join("npm-cache"),
            );

            // 配置了自动启动时，随应用启动本地代理（端口被占用会自动顺延）
            auto_start_proxy(&app_handle, &db_path);

            // 配置了自动启动时，随应用启动局域网访问服务（enabled=1）
            auto_start_lan(&app_handle, &db_path);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::command_palette::list_palette_commands,
            commands::project::list_projects,
            commands::project::add_project,
            commands::project::scan_workspace_modules,
            commands::project::rescan_workspace_modules,
            commands::project::set_workspace_modules,
            commands::project::get_harmony_root,
            commands::project::set_harmony_project_path,
            commands::project::inspect_project,
            commands::project::trust_project,
            commands::project::delete_project,
            commands::project::project_scoped_counts,
            commands::project::list_conversations,
            commands::project::get_conversation,
            commands::project::create_conversation,
            commands::project::fork_conversation,
            commands::project::update_conversation,
            commands::project::list_conversations_by_tag,
            commands::project::list_conversation_tags,
            commands::project::list_messages,
            commands::project::list_messages_page,
            commands::project::search_messages,
            commands::project::search_messages_all_projects,
            commands::project::send_message,
            commands::chat::stream_chat,
            commands::chat::stop_chat,
            commands::chat::stop_tool,
            commands::chat::list_tool_whitelist,
            commands::chat::remove_tool_whitelist,
            commands::chat::queue_message,
            commands::chat::list_queued_messages,
            commands::chat::remove_queued_message,
            commands::chat::update_message,
            commands::chat::delete_message,
            commands::chat::list_snapshots,
            commands::chat::restore_snapshot,
            commands::chat::resolve_tool_approval,
            commands::chat::resolve_diagnose_card,
            commands::chat::resolve_plan_review,
            commands::chat::resolve_ask_user,
            commands::chat::get_todos,
            commands::chat::get_task_ledger,
            commands::chat::get_ask,
            commands::chat::list_pending_confirmations,
            commands::chat::rename_conversation,
            commands::chat::compact_conversation,
            commands::chat::conversation_context,
            commands::chat::get_session_events,
            commands::chat::get_latest_agent_run,
            commands::chat::get_agent_run_events,
            commands::chat::get_agent_run_steps,
            commands::chat::delete_conversation,
            commands::chat::save_message_feedback,
            commands::chat::list_message_feedback,
            commands::chat::list_message_versions,
            commands::chat::summarize_memory,
            commands::chat::conversation_cost_stats,
            commands::project::get_git_branches,
            commands::project::switch_git_branch,
            commands::git::git_discover_repos,
            commands::git::git_branch_info,
            commands::git::git_switch_branch,
            commands::git::git_worktree_list,
            commands::git::git_worktree_create,
            commands::git::git_worktree_remove,
            commands::git::set_project_worktree,
            commands::git::git_worktree_merge,
            commands::git::rollback_conversation,
            commands::git::git_file_diff,
            commands::git::git_accept_changes,
            commands::git::git_revert_file,
            commands::git::git_diff_stat,
            commands::preview::open_preview_window,
            commands::terminal::open_terminal,
            commands::terminal::terminal_exec,
            commands::terminal::terminal_kill,
            commands::terminal::terminal_status,
            commands::rules::get_global_rules,
            commands::rules::set_global_rules,
            commands::rules::update_project_rules,
            commands::limits::get_agent_limits,
            commands::limits::set_agent_limits,
            commands::limits::reset_agent_limits,
            commands::index::build_project_index,
            commands::index::get_project_file_tree,
            commands::index::list_project_dir,
            commands::index::search_project_files,
            commands::index::read_project_file,
            commands::index::save_project_file,
            commands::index::delete_project_file,
            commands::index::index_project_symbols,
            commands::index::refresh_project_symbols,
            commands::index::project_outline,
            commands::index::search_symbols,
            commands::index::warmup_symbol_index,
            commands::index::symbol_index_meta,
            commands::index::symbol_counts,
            commands::provider::list_providers,
            commands::provider::list_provider_models,
            commands::provider::create_provider,
            commands::provider::update_provider,
            commands::provider::delete_provider,
            commands::provider::switch_provider,
            commands::provider::test_provider,
            commands::provider::update_model,
            commands::provider::add_model,
            commands::provider::remove_model,
            commands::provider::reorder_provider_models,
            commands::provider::reorder_providers,
            commands::provider::sync_provider_models,
            commands::version::get_current_version,
            commands::version::list_available_versions,
            commands::version::install_version,
            commands::version::check_base_update,
            commands::config::read_config,
            commands::config::write_config,
            commands::config::get_config_path,
            commands::cost::get_cost_summary,
            commands::cost::get_request_logs,
            commands::cost::get_daily_usage,
            commands::cost::get_task_stats,
            commands::cost::get_task_runs,
            commands::cost::get_budget_status,
            commands::cost::get_all_budget_status,
            commands::reliability::get_reliability_dashboard,
            commands::reliability::run_reliability_evaluation,
            commands::reliability::list_scheduled_agent_tasks,
            commands::reliability::list_agent_dag_nodes,
            commands::reliability::list_agent_alerts,
            commands::reliability::list_agent_audit_events,
            commands::reliability::list_agent_workers,
            commands::reliability::get_scheduled_agent_task,
            commands::reliability::claim_next_scheduled_agent_task,
            commands::reliability::retry_scheduled_agent_task,
            commands::reliability::resume_scheduled_agent_task,
            commands::reliability::add_agent_dag_dependency,
            commands::reliability::list_runnable_agent_dag_nodes,
            commands::reliability::retry_agent_dag_node,
            commands::reliability::cancel_agent_dag_descendants,
            commands::reliability::get_agent_slo_policy,
            commands::reliability::update_agent_slo_policy,
            commands::devices::list_devices,
            commands::devices::set_default_device,
            commands::devices::get_device_detail,
            commands::devices::hdc_available,
            commands::devices::start_hdc_service,
            commands::devices::stop_hdc_service,
            commands::devices::capture_device_screenshot,
            commands::devices::list_device_screenshots,
            commands::devices::delete_device_screenshot,
            commands::devices::list_installed_apps,
            commands::devices::launch_app,
            commands::devices::stop_app,
            commands::devices::list_device_processes,
            commands::devices::start_hilog_stream,
            commands::devices::stop_hilog_stream,
            commands::devices::get_device_perf,
            commands::index::search_symbols_all,
            commands::mcp::list_mcp_servers,
            commands::mcp::add_mcp_server,
            commands::mcp::update_mcp_server,
            commands::mcp::test_mcp_server,
            commands::mcp::toggle_mcp_server,
            commands::mcp::remove_mcp_server,
            commands::mcp::clone_mcp_server,
            commands::mcp::export_mcp_config,
            commands::mcp::import_mcp_config,
            commands::mcp::fetch_mcp_from_url,
            commands::mcp::list_mcp_usage_stats,
            commands::skill::list_skills,
            commands::skill::import_skill_from_github,
            commands::skill::toggle_skill,
            commands::skill::remove_skill,
            commands::skill::clone_skill,
            commands::skill::list_skill_usage,
            commands::skill::list_skill_usage_events,
            commands::knowledge::list_knowledge,
            commands::knowledge::add_knowledge,
            commands::knowledge::update_knowledge,
            commands::knowledge::toggle_knowledge,
            commands::knowledge::delete_knowledge,
            commands::knowledge::clone_knowledge,
            commands::knowledge::save_knowledge_from_text,
            // 鸿蒙官方 API 知识库管理
            commands::api_knowledge::api_kb_stats,
            commands::api_knowledge::api_kb_filters,
            commands::api_knowledge::api_docs_list,
            commands::api_knowledge::api_details_list,
            commands::api_knowledge::api_detail_get,
            commands::api_knowledge::api_doc_add,
            commands::api_knowledge::api_doc_delete,
            commands::api_knowledge::api_detail_upsert,
            commands::api_knowledge::api_detail_delete,
            commands::api_knowledge::api_kb_clear,
            commands::api_knowledge::api_kb_refresh_docs,
            commands::api_knowledge::api_kb_refresh_details,
            commands::api_knowledge::api_kb_embed_status,
            commands::api_knowledge::api_kb_embed_index,
            // ohpm 三方库推荐缓存（官方 landscape 镜像）
            commands::ohpm_landscape::ohpm_landscape_status,
            commands::ohpm_landscape::ohpm_landscape_refresh,
            commands::ohpm_landscape::ohpm_landscape_search,
            commands::ohpm_landscape::ohpm_landscape_hot,
            commands::ohpm_landscape::ohpm_landscape_by_category,
            commands::ohpm_landscape::ohpm_landscape_count,
            commands::ohpm_landscape::ohpm_landscape_categories,
            commands::ohpm_landscape::ohpm_landscape_repo_url,
            commands::memory::list_memories,
            commands::memory::save_memory,
            commands::memory::delete_memory,
            commands::memory::set_memory_enabled,
            commands::memory::list_tool_stats,
            commands::memory::list_tool_token_stats,
            commands::tools::list_tool_groups,
            commands::tools::tools_health,
            commands::maintenance::clear_content_data,
            commands::maintenance::run_maintenance,
            commands::maintenance::data_scale,
            commands::maintenance::export_backup,
            commands::health::check_all_health,
            commands::health::check_harmony_toolchain,
            commands::health::get_toolchain_candidates,
            commands::harmony_analyze::analyze_build_errors,
            commands::harmony_analyze::analyze_generic_project,
            commands::harmony_analyze::analyze_harmony_project,
            commands::harmony_analyze::check_ohpm_deps,
            commands::harmony_analyze::run_ohpm_install,
            commands::balance::query_balances,
            commands::node_runtime::get_node_runtime,
            commands::node_runtime::upgrade_node_runtime,
            commands::node_runtime::reset_node_runtime,
            commands::jdk_runtime::get_jdk_runtime,
            commands::jdk_runtime::fetch_jdk_releases,
            commands::jdk_runtime::install_jdk,
            commands::jdk_runtime::check_jdk_updates,
            commands::jdk_runtime::set_default_jdk,
            commands::jdk_runtime::uninstall_jdk,
            commands::proxy::start_proxy,
            commands::proxy::stop_proxy,
            commands::proxy::get_proxy_status,
            commands::proxy::get_proxy_config,
            commands::proxy::update_proxy_config,
            // 局域网访问（LAN Access）
            commands::lan::start_lan_server,
            commands::lan::stop_lan_server,
            commands::lan::get_lan_server_status,
            commands::lan::update_lan_server_config,
            commands::lan::create_lan_token,
            commands::lan::list_lan_tokens,
            commands::lan::revoke_lan_token,
            commands::lan::get_lan_ips,
            commands::update::begin_update_proxy,
            commands::update::end_update_proxy,
            commands::update::get_system_proxy,
            commands::environment::get_app_info,
            commands::environment::get_environment_info,
            commands::environment::fetch_node_latest_lts,
            commands::environment::fetch_git_latest_version,
            commands::environment::get_git_runtime,
            commands::environment::upgrade_git_runtime,
            commands::environment::reset_git_runtime,
            commands::environment::install_toolkit,
            commands::environment::install_toolkit_from_zip,
            commands::environment::get_tool_version,
            services::harmony_env::get_harmony_env,
            services::harmony_env::detect_harmony_env,
            services::harmony_env::save_harmony_env,
            services::harmony_env::list_sdk_api_modules,
            services::harmony_env::search_sdk_api,
            services::harmony_env::read_sdk_api_module,
            services::harmony_env::check_project_sdk_alignment,
            services::harmony_env::get_harmony_docs_status,
            services::harmony_env::update_harmony_docs,
            services::harmony_env::search_harmony_docs,
            services::harmony_env::read_harmony_doc,
            commands::desktop::detect_system_locale,
            commands::desktop::send_notification,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // 退出应用时停止本地代理（仅代理锁持有者；多开时其他实例共享代理，退出不影响）
            if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
                let handle = app_handle.clone();
                tauri::async_runtime::block_on(async move {
                    if let Some(db) = handle.try_state::<db::DbState>() {
                        if let Ok(conn) = db.0.try_lock() {
                            let _ = crate::agent::scheduler::stop_current_worker(&conn);
                        }
                    }
                    // MCP 子进程属于当前桌面 Worker，每个实例都应独立回收。
                    if let Some(mcp) = handle.try_state::<services::mcp_manager::McpManager>() {
                        mcp.shutdown_all();
                    }
                    let is_owner = handle
                        .try_state::<ProxyLock>()
                        .map(|l| l.0.try_lock().map(|g| g.is_some()).unwrap_or(false))
                        .unwrap_or(false);
                    if !is_owner {
                        return;
                    }
                    if let Some(state) = handle.try_state::<ProxyState>() {
                        let mut server = state.0.lock().await;
                        let _ = server.stop().await;
                    }
                    // 停止局域网访问服务（多开时仅锁持有者启动过；与代理同分支）
                    if let Some(state) = handle.try_state::<commands::lan::LanServerState>() {
                        let mut server = state.0.lock().await;
                        let _ = server.stop().await;
                    }
                });
            }
        });
}
