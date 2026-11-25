use super::{proto::Message, storage::{MODEL_RESPONSE, TOOL_RESULT, USER_PROMPT}};
use serde_json::{json, Value};

const ERROR: i64 = 17;
const CHECKPOINT: i64 = 23;
const SYSTEM_NOTICE: i64 = 101;

pub(super) const DETAIL_TYPES: [i64; 4] = [TOOL_RESULT, ERROR, CHECKPOINT, SYSTEM_NOTICE];

pub(super) fn is_notice(kind: i64) -> bool {
    matches!(kind, ERROR | CHECKPOINT | SYSTEM_NOTICE)
}

pub(super) enum Content {
    Message(Vec<Value>),
    /// 其他会话发来的消息；派生子智能体时父会话发出的首条消息是任务。
    Agent { kind: &'static str, from: String, text: Option<String> },
    Context { kind: &'static str, label: &'static str, text: String },
    System(String),
}

pub(super) fn decode(
    kind: i64,
    metadata: &Message<'_>,
    payload: &[u8],
    status: Option<i64>,
    error: &[u8],
    detail: bool,
) -> std::io::Result<Content> {
    let payload = Message::decode(payload)?;
    Ok(match kind {
        USER_PROMPT => Content::Message(prompt(&payload.message(19)?, detail)?),
        MODEL_RESPONSE => Content::Message(response(&payload.message(20)?, detail)?),
        TOOL_RESULT => Content::Message(tool_result(metadata, &payload, status, error)?),
        ERROR => {
            let notice = payload.message(24)?.message(3)?;
            let text = notice.text(2).or_else(|| notice.text(1)).or_else(|| notice.text(3));
            Content::System(text.unwrap_or("Antigravity 执行出错").into())
        }
        CHECKPOINT => Content::Context {
            kind: "compaction",
            label: "压缩摘要",
            text: payload.message(30)?.text(5).unwrap_or("已记录会话检查点").into(),
        },
        SYSTEM_NOTICE => notice(&payload.message(114)?)?,
        _ => Content::Message(Vec::new()),
    })
}

fn notice(notice: &Message<'_>) -> std::io::Result<Content> {
    if notice.text(3) != Some("agent_message") {
        return Ok(Content::Context {
            kind: "runtime",
            label: "系统记录",
            text: notice.text(1).unwrap_or("收到系统通知").into(),
        });
    }
    let message = notice.message(4)?;
    let spawned = message.message(12).and_then(|source| source.message(1)).and_then(|source| source.message(3))
        .is_ok_and(|call| call.text(2) == Some("invoke_subagent"));
    Ok(Content::Agent {
        kind: if spawned { "task" } else { "message" },
        from: message.text(3).unwrap_or_default().into(),
        text: message.text(4).map(str::to_owned),
    })
}

fn prompt(prompt: &Message<'_>, detail: bool) -> std::io::Result<Vec<Value>> {
    let segments = prompt.repeated(3).map(Message::decode)
        .collect::<Result<Vec<_>, _>>()?;
    let texts: Vec<_> = segments.iter().filter_map(|segment| segment.text(1)).collect();
    let text = match prompt.text(2) {
        Some(text) if texts.iter().all(|segment| text.contains(segment)) => text.to_owned(),
        _ => texts.join("\n"),
    };
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(json!({"type":"text","text":text}));
    }
    for data in prompt.repeated(9) {
        let attachment = Message::decode(data)?;
        let mime = attachment.text(1).unwrap_or("application/octet-stream");
        let kind = if mime.starts_with("image/") { "image" } else { "file" };
        let mut block = json!({"type":kind});
        if detail {
            if let Some(bytes) = attachment.repeated(2).next_back() {
                block["source"] = json!({"type":"base64","media_type":mime,"data":base64(bytes)});
            }
        }
        blocks.push(block);
    }
    Ok(blocks)
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let [a, b, c] = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        output.push(ALPHABET[(a >> 2) as usize] as char);
        output.push(ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 { ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize] as char } else { '=' });
        output.push(if chunk.len() > 2 { ALPHABET[(c & 63) as usize] as char } else { '=' });
    }
    output
}

fn response(response: &Message<'_>, detail: bool) -> std::io::Result<Vec<Value>> {
    let mut blocks = Vec::new();
    if let Some(thinking) = response.text(3) {
        blocks.push(json!({"type":"thinking","thinking":thinking}));
    }
    if let Some(text) = response.text(1).or_else(|| response.text(8)) {
        blocks.push(json!({"type":"text","text":text}));
    }
    for call in response.repeated(7) {
        let call = Message::decode(call)?;
        if let Some(name) = call.text(2) {
            let mut block = json!({"type":"tool_use","id":call.text(1),"name":name});
            if detail {
                block["input"] = call.text(3)
                    .map(|text| serde_json::from_str(text).unwrap_or_else(|_| json!(text)))
                    .unwrap_or_else(|| json!({}));
            }
            blocks.push(block);
        }
    }
    Ok(blocks)
}

fn tool_result(
    metadata: &Message<'_>,
    payload: &Message<'_>,
    status: Option<i64>,
    error: &[u8],
) -> std::io::Result<Vec<Value>> {
    let call = metadata.message(4)?;
    let output = payload.message(140)?.message(2)?;
    let background = payload.message(148)?;
    let error_details = Message::decode(error)?;
    let text = output.text(1).or_else(|| error_details.text(2))
        .or_else(|| error_details.text(3)).or_else(|| output.text(3))
        .or_else(|| background.text(4)).or_else(|| background.text(3))
        .or_else(|| background.text(7));
    let status = match status {
        Some(2) => Some("running"),
        Some(3) => Some("completed"),
        Some(6) => Some("cancelled"),
        Some(7) => Some("error"),
        _ if !error.is_empty() => Some("error"),
        _ => None,
    };
    if text.is_none() && (call.text(1).is_none() || status.is_none()) {
        return Ok(Vec::new());
    }
    let text = match (status, text) {
        (Some("cancelled"), Some(text)) => Some(format!("工具执行已取消\n\n{text}")),
        (Some("cancelled"), None) => Some("工具执行已取消".into()),
        (Some("error"), None) => Some("工具执行失败".into()),
        (_, text) => text.map(str::to_owned),
    };
    Ok(vec![json!({"type":"tool_result","tool_use_id":call.text(1),
        "content":text,"status":status,"is_error":status == Some("error")})])
}
