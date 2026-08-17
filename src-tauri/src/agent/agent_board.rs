//! Agent 消息板（轻量 A2A）：会话级 publish/subscribe，供子 Agent 之间与主 Agent 传递中间结果。
//! 与 ask/todo 同模式：OnceLock 全局静态表，不落库（重启清空），每会话保留最近 N 条。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// 一条消息板消息
#[derive(Clone, serde::Serialize)]
pub struct BoardMessage {
    /// 主题（publish 时指定，subscribe 按主题过滤）
    pub topic: String,
    /// 消息内容
    pub content: String,
    /// 发送者标识（工具名/子 Agent 名）
    pub sender: String,
    /// 时间戳（unix 秒）
    pub ts: i64,
}

static BOARD: OnceLock<Mutex<HashMap<String, Vec<BoardMessage>>>> = OnceLock::new();

fn table() -> &'static Mutex<HashMap<String, Vec<BoardMessage>>> {
    BOARD.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 每会话消息上限（超出丢弃最旧，防内存无限增长）
const MAX_PER_CONV: usize = 200;

/// 发布一条消息；返回该 topic 当前累计条数
pub fn publish(conversation_id: &str, topic: &str, content: &str, sender: &str) -> usize {
    let mut t = table().lock().unwrap_or_else(|p| p.into_inner());
    let v = t.entry(conversation_id.to_string()).or_default();
    v.push(BoardMessage {
        topic: topic.to_string(),
        content: content.to_string(),
        sender: sender.to_string(),
        ts: chrono::Local::now().timestamp(),
    });
    if v.len() > MAX_PER_CONV {
        let excess = v.len() - MAX_PER_CONV;
        v.drain(..excess);
    }
    v.iter().filter(|m| m.topic == topic).count()
}

/// 订阅：读取指定 topic 的消息（新 → 旧，最多 limit 条）；topic 为空返回全部
pub fn subscribe(conversation_id: &str, topic: &str, limit: usize) -> Vec<BoardMessage> {
    let t = table().lock().unwrap_or_else(|p| p.into_inner());
    let Some(v) = t.get(conversation_id) else {
        return Vec::new();
    };
    let limit = limit.clamp(1, 100);
    v.iter()
        .rev()
        .filter(|m| topic.is_empty() || m.topic == topic)
        .take(limit)
        .cloned()
        .collect()
}

/// 清空某会话消息板（Agent 主动清理用），返回清除条数；
/// 暂未接入工具（消息板自带容量上限），保留供后续流水线场景调用
#[allow(dead_code)]
pub fn clear(conversation_id: &str) -> usize {
    let mut t = table().lock().unwrap_or_else(|p| p.into_inner());
    t.remove(conversation_id).map(|v| v.len()).unwrap_or(0)
}

/// agent_publish 工具：发布消息到会话消息板
pub(super) async fn agent_publish(
    args: &serde_json::Value,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let topic = args["topic"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("agent_publish 需要参数 {\"topic\":\"<主题>\",\"content\":\"<消息内容>\"}")?;
    let content = args["content"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("agent_publish 需要参数 {\"topic\":\"<主题>\",\"content\":\"<消息内容>\"}")?;
    if content.chars().count() > 4000 {
        return Err("消息内容过长（>4000 字符），请精简后再发布".into());
    }
    let n = publish(&ctx.conversation_id, topic, content, "agent_publish");
    Ok(format!(
        "已发布到 topic「{topic}」（该主题累计 {n} 条）。其他子 Agent / 主 Agent 可调用 agent_subscribe(topic=\"{topic}\") 读取。"
    ))
}

/// agent_subscribe 工具：读取会话消息板上指定 topic 的消息
pub(super) async fn agent_subscribe(
    args: &serde_json::Value,
    ctx: &crate::agent::exec_ctx::ToolCtx,
) -> Result<String, String> {
    let topic = args["topic"].as_str().unwrap_or("");
    let limit = args["limit"].as_u64().unwrap_or(20) as usize;
    let msgs = subscribe(&ctx.conversation_id, topic, limit);
    if msgs.is_empty() {
        return Ok(if topic.is_empty() {
            "消息板上暂无消息".into()
        } else {
            format!("topic「{topic}」暂无消息（可稍后重试，或检查 topic 名是否与发布者一致）")
        });
    }
    let mut out = format!("消息板共 {} 条（新→旧）：", msgs.len());
    for m in msgs {
        let t = chrono::DateTime::from_timestamp(m.ts, 0)
            .map(|d| d.format("%H:%M:%S").to_string())
            .unwrap_or_default();
        out.push_str(&format!(
            "\n[{t} · topic={} · {}] {}",
            m.topic, m.sender, m.content
        ));
    }
    Ok(out)
}
