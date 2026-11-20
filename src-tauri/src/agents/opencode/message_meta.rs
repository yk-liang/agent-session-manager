//! 已保存的助手消息提供模型和客户端标识。
use super::storage;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashMap;

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
pub(super) fn from_message(message: &Value) -> Option<String> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    model_id(message)
}

pub(crate) fn model_id(message: &Value) -> Option<String> {
    let nested = message.get("model");
    let model = text(message.get("modelID"))
        .or_else(|| text(nested.and_then(|v| v.get("modelID").or_else(|| v.get("id")))))
        .or_else(|| text(nested))?;
    let provider =
        text(message.get("providerID")).or_else(|| text(nested.and_then(|v| v.get("providerID"))));
    Some(match provider {
        Some(provider) if model.split_once('/').is_none_or(|(prefix, _)| prefix != provider) => {
            format!("{provider}/{model}")
        }
        _ => model.to_owned(),
    })
}

pub(crate) fn client_id(message: &Value) -> Option<String> {
    text(message.get("agent"))
        .or_else(|| text(message.get("mode")))
        .map(|name| name.chars().take(2048).collect())
}

#[derive(Default)]
pub(super) struct MessageMetadata {
    pub model: Option<String>,
    pub client: Option<String>,
}

/// 一次读取轻量消息元数据，不连接 part 表，也不逐会话扫描工具输出。
pub(super) fn latest(
    conn: &Connection,
    session: Option<&str>,
    warnings: &mut Vec<String>,
) -> HashMap<String, MessageMetadata> {
    let filter = if session.is_some() {
        "session_id=?1 AND "
    } else {
        ""
    };
    let order = storage::order_column(conn);
    let query = format!(
        "SELECT session_id,json_object('role','assistant','modelID',json_extract(data,'$.modelID'),
        'providerID',json_extract(data,'$.providerID'),'model',json_extract(data,'$.model'),
        'agent',json_extract(data,'$.agent'),'mode',json_extract(data,'$.mode'))
        FROM message WHERE {filter}json_valid(data) AND json_extract(data,'$.role')='assistant'
        ORDER BY {order},id"
    );
    let read = || -> Result<HashMap<String, MessageMetadata>, rusqlite::Error> {
        let mut stmt = conn.prepare(&query)?;
        let decode =
            |row: &rusqlite::Row<'_>| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?));
        let rows = if let Some(id) = session {
            stmt.query_map([id], decode)?
        } else {
            stmt.query_map([], decode)?
        };
        let mut messages = HashMap::<String, MessageMetadata>::new();
        for row in rows {
            let (id, data) = row?;
            if let Ok(value) = serde_json::from_str::<Value>(&data) {
                let meta = messages.entry(id).or_default();
                if let Some(model) = from_message(&value) {
                    meta.model = Some(model);
                }
                if let Some(client) = client_id(&value) {
                    meta.client = Some(client);
                }
            }
        }
        Ok(messages)
    };
    read().unwrap_or_else(|_| {
        warnings.push("OpenCode 历史模型读取失败，当前保留其他会话信息".into());
        HashMap::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_or_nested_model_fields_do_not_use_user_requested_models() {
        assert!(from_message(&json!({"role":"user","model":{"modelID":"not-used"}})).is_none());
        assert_eq!(
            from_message(
                &json!({"role":"assistant","model":{"providerID":"legacy","modelID":"id"}})
            )
            .unwrap(),
            "legacy/id"
        );
        assert_eq!(
            from_message(&json!({"role":"assistant","modelID":"local"})).unwrap(),
            "local"
        );
        assert!(from_message(&json!({"role":"assistant","modelID":42})).is_none());
    }
}
