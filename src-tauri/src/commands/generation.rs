//! 生成媒体命令：generate_media —— 校验会话/Provider → 选生成模型（显式指定校验
//! output_modalities 或按模态自动路由）→ TaskRegistry 注册（可被停止/看门狗兜底）→
//! 调用 generation 服务 → 结果以媒体标记入库 assistant 消息 → 推送
//! gen-start / gen-progress / gen-done / gen-error 事件。
//!
//! 消息 content 媒体标记：`![](<绝对路径>)`（图片）、`![VIDEO](<绝对路径>)`、
//! `![AUDIO](<绝对路径>)`；前端 Markdown 组件据此渲染图片/播放器。

use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::agent::tools::guards::is_cancelled;
use crate::commands::chat::ChatCancel;
use crate::db::models::ChatMessage;
use crate::db::DbState;
use crate::services::generation::{self, GenKind, GenRequest};
use crate::utils::task_registry::{TaskRegistry, PHASE_TOOL};

/// 生成开始事件
#[derive(Clone, Serialize)]
struct GenStartEvent {
    conversation_id: String,
    kind: String,
}

/// 生成进度事件（视频异步轮询期间周期推送）
#[derive(Clone, Serialize)]
struct GenProgressEvent {
    conversation_id: String,
    kind: String,
    stage: String,
    waited_secs: u64,
}

/// 生成完成事件（携带已入库的 assistant 消息）
#[derive(Clone, Serialize)]
struct GenDoneEvent {
    conversation_id: String,
    message: ChatMessage,
}

/// 生成失败事件
#[derive(Clone, Serialize)]
struct GenErrorEvent {
    conversation_id: String,
    error: String,
}

/// 任务监管守卫：正常收尾注销；Drop 时 abort + 条件注销（防幽灵任务占住注册表）
struct RegisteredGenTask {
    app: AppHandle,
    conversation_id: String,
    generation: u64,
    abort: tokio::task::AbortHandle,
    armed: bool,
}

impl RegisteredGenTask {
    fn finish(&mut self) {
        self.app
            .state::<TaskRegistry>()
            .unregister(&self.conversation_id, self.generation);
        self.armed = false;
    }
}

impl Drop for RegisteredGenTask {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // WebView 重载/窗口销毁会取消 invoke Future：同步 abort + 条件注销
        self.abort.abort();
        self.app
            .state::<TaskRegistry>()
            .unregister(&self.conversation_id, self.generation);
    }
}

/// 生成媒体：kind ∈ image | video | audio；prompt 为生成描述；model_id 可选（缺省按
/// 当前激活 Provider 自动路由）；images 为参考图（data URL，视频生图参考帧）。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn generate_media(
    app: AppHandle,
    state: State<'_, DbState>,
    _cancel: State<'_, ChatCancel>,
    conversation_id: String,
    kind: String,
    prompt: String,
    model_id: Option<String>,
    images: Option<Vec<String>>,
) -> Result<(), String> {
    let gen_kind = GenKind::parse(&kind)
        .ok_or_else(|| format!("不支持的生成类型: {kind}（应为 image/video/audio）"))?;
    if prompt.trim().is_empty() {
        return Err("请输入生成描述".to_string());
    }
    {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(1) FROM conversations WHERE id = ?1",
                [&conversation_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists == 0 {
            return Err("会话不存在".into());
        }
    }

    // 任务监管壳（与 stream_chat 一致）：spawn 取得 AbortHandle → 唯一登记 → 放行主体
    let conv_for_spawn = conversation_id.clone();
    let app_for_spawn = app.clone();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        if start_rx.await.is_err() {
            return Err("任务启动已取消".to_string());
        }
        let state = app_for_spawn.state::<DbState>();
        let cancel = app_for_spawn.state::<ChatCancel>();
        let registry = app_for_spawn.state::<TaskRegistry>();
        generate_media_body(
            &app_for_spawn,
            &state,
            &cancel,
            &registry,
            conv_for_spawn,
            gen_kind,
            prompt,
            model_id,
            images,
        )
        .await
    });
    let abort_handle = join.abort_handle();
    let generation = {
        let registry = app.state::<TaskRegistry>();
        let Some(generation) = registry.register(&conversation_id, abort_handle.clone()) else {
            join.abort();
            return Err("该会话已有任务进行中，请等待完成或停止后重试".to_string());
        };
        TaskRegistry::ensure_watchdog(app.clone());
        generation
    };
    let mut registered = RegisteredGenTask {
        app: app.clone(),
        conversation_id: conversation_id.clone(),
        generation,
        abort: abort_handle,
        armed: true,
    };
    let _ = start_tx.send(());
    let result = join.await;
    registered.finish();
    match result {
        Ok(r) => r,
        Err(e) if e.is_cancelled() => Err("任务已被强制终止（看门狗判定异常卡死或停止未生效）。请重试。".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// 生成任务主体：选模型 → 调生成服务 → 结果入库 → 事件推送
#[allow(clippy::too_many_arguments)]
async fn generate_media_body(
    app: &AppHandle,
    state: &State<'_, DbState>,
    cancel: &State<'_, ChatCancel>,
    registry: &TaskRegistry,
    conversation_id: String,
    kind: GenKind,
    prompt: String,
    model_id: Option<String>,
    images: Option<Vec<String>>,
) -> Result<(), String> {
    let kind_label = kind.label();
    let _ = app.emit(
        "gen-start",
        GenStartEvent {
            conversation_id: conversation_id.clone(),
            kind: kind_label.to_string(),
        },
    );

    // 1. 选模型（显式指定并校验 output_modalities，或按模态自动路由）
    let (provider_id, model, use_proxy) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        resolve_generation_model(&conn, model_id.as_deref(), modality_of(kind))?
    };
    // 2. Provider 端点
    let (base_url, api_key) = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        load_provider_endpoint(&conn, &provider_id)?
    };
    let client = crate::utils::net::build_client(use_proxy)?;

    // 3. 产物目录：{app_data}/generated/{conversation_id}/{uuid}（每次请求独立子目录防覆盖）
    let out_dir = app
        .path()
        .app_data_dir()
        .unwrap_or_default()
        .join("generated")
        .join(&conversation_id)
        .join(uuid::Uuid::new_v4().to_string());

    // 停止检查：复用会话级 ChatCancel（前端停止按钮写入同一集合）
    let conv_for_stop = conversation_id.clone();
    let is_stopped = move || is_cancelled(cancel, &conv_for_stop);
    // 视频轮询进度：每轮 touch 心跳（防看门狗 8 分钟无心跳误杀）并推送进度
    let app2 = app.clone();
    let conv_for_progress = conversation_id.clone();
    let on_progress = move |waited_secs: u64| {
        registry.touch(&conv_for_progress, PHASE_TOOL);
        let _ = app2.emit(
            "gen-progress",
            GenProgressEvent {
                conversation_id: conv_for_progress.clone(),
                kind: kind_label.to_string(),
                stage: "polling".to_string(),
                waited_secs,
            },
        );
    };

    // 4. 调用生成服务（图片/音频同步；视频异步轮询，轮询期间可停止）
    let files = {
        let req = GenRequest {
            client: &client,
            base_url: &base_url,
            api_key: api_key.as_deref(),
            kind,
            model: &model,
            prompt: &prompt,
            images: images.as_deref().unwrap_or(&[]),
            out_dir: &out_dir,
            is_stopped: &is_stopped,
            on_progress: &on_progress,
        };
        match generation::generate(&req).await {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("{}生成失败: {e}", kind_label);
                let _ = app.emit(
                    "gen-error",
                    GenErrorEvent {
                        conversation_id: conversation_id.clone(),
                        error: msg.clone(),
                    },
                );
                return Err(msg);
            }
        }
    };

    // 5. 结果入库（assistant 消息，content 带媒体标记）+ gen-done
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let message = match persist_generation_message(&conn, &conversation_id, &model, kind, &files) {
        Ok(m) => m,
        Err(e) => {
            let _ = app.emit(
                "gen-error",
                GenErrorEvent {
                    conversation_id: conversation_id.clone(),
                    error: e.clone(),
                },
            );
            return Err(e);
        }
    };
    let _ = app.emit(
        "gen-done",
        GenDoneEvent {
            conversation_id: conversation_id.clone(),
            message,
        },
    );
    Ok(())
}

/// 输出模态字符串（与 output_modalities JSON 数组元素对应）
fn modality_of(kind: GenKind) -> &'static str {
    match kind {
        GenKind::Image => "image",
        GenKind::Video => "video",
        GenKind::Audio => "audio",
    }
}

/// 模态中文名（错误提示）
fn modality_label(m: &str) -> &'static str {
    match m {
        "image" => "图片",
        "video" => "视频",
        "audio" => "音频",
        _ => "媒体",
    }
}

/// output_modalities JSON 数组是否含目标模态（容错子串匹配）
fn modalities_include(json: &str, want: &str) -> bool {
    json.contains(&format!("\"{want}\"")) || json == want
}

/// 解析生成模型：
/// - model_id 指定：跨 Provider 查询并校验 output_modalities 含目标模态；
/// - 缺省：当前激活 Provider 内 pick_model_for_output 自动路由（落空返回明确错误）。
///
/// 返回 (provider_id, model_id, use_proxy)。
fn resolve_generation_model(
    conn: &Connection,
    model_id: Option<&str>,
    modality: &str,
) -> Result<(String, String, bool), String> {
    if let Some(mid) = model_id {
        let (model, provider_id, use_proxy, output_modalities) = conn
            .query_row(
                "SELECT m.model_id, m.provider_id, m.use_proxy, m.output_modalities
                 FROM models m WHERE m.id = ?1 AND m.enabled = 1",
                [mid],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(|e| format!("指定模型不存在或已停用: {e}"))?;
        if !modalities_include(&output_modalities, modality) {
            return Err(format!(
                "模型 {model} 不支持{}生成（output_modalities 缺少 \"{modality}\"）",
                modality_label(modality)
            ));
        }
        Ok((provider_id, model, use_proxy))
    } else {
        let provider_id: String = conn
            .query_row(
                "SELECT id FROM providers WHERE is_active = 1 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| "没有激活的 Provider，请先在设置中添加并启用服务商".to_string())?;
        let model = crate::services::model_router::pick_model_for_output(conn, &provider_id, modality)
            .ok_or_else(|| {
                format!(
                    "当前服务商没有可用的{}生成模型，请在模型设置中启用/添加支持{}输出的模型",
                    modality_label(modality),
                    modality_label(modality)
                )
            })?;
        let use_proxy: bool = conn
            .query_row(
                "SELECT use_proxy FROM models WHERE provider_id = ?1 AND model_id = ?2",
                params![provider_id, model],
                |r| r.get(0),
            )
            .unwrap_or(false);
        Ok((provider_id, model, use_proxy))
    }
}

/// 读取 Provider 端点（API Key 统一走安全读取：数据库明文优先，其次系统凭据管理器）
fn load_provider_endpoint(
    conn: &Connection,
    provider_id: &str,
) -> Result<(String, Option<String>), String> {
    let (base_url, api_key) = conn
        .query_row(
            "SELECT base_url, api_key FROM providers WHERE id = ?1",
            [provider_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .map_err(|e| format!("Provider 配置读取失败: {e}"))?;
    let key = crate::services::key_store::load_provider_key(conn, provider_id)?.or(api_key);
    Ok((base_url, key))
}

/// 生成结果入库：content = 媒体标记（图片/视频/音频），返回完整消息对象
fn persist_generation_message(
    conn: &Connection,
    conversation_id: &str,
    model: &str,
    kind: GenKind,
    files: &[String],
) -> Result<ChatMessage, String> {
    let mut content = String::new();
    for f in files {
        let marker = match kind {
            GenKind::Image => format!("![]({f})"),
            GenKind::Video => format!("![VIDEO]({f})"),
            GenKind::Audio => format!("![AUDIO]({f})"),
        };
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str(&marker);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO messages (id, conversation_id, role, content, model, created_at)
         VALUES (?1, ?2, 'assistant', ?3, ?4, ?5)",
        params![id, conversation_id, content, model, created_at],
    )
    .map_err(|e| format!("生成结果入库失败: {e}"))?;
    Ok(ChatMessage {
        id,
        conversation_id: conversation_id.to_string(),
        role: "assistant".to_string(),
        content,
        references_json: None,
        model: Some(model.to_string()),
        tokens_in: None,
        tokens_out: None,
        created_at,
        reasoning: None,
        queued: 0,
        agent_owned: 0,
        modified_files_json: None,
        duration_ms: None,
    })
}
