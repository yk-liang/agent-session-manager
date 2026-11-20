//! 工具结果可保存为旧版字符串或 Responses 内容块数组。
use serde::Deserialize;
use serde_json::{value::RawValue, Value};

#[derive(Deserialize)]
struct OutputStatus<'a> {
    #[serde(borrow)]
    metadata: Option<&'a RawValue>,
    #[serde(borrow)]
    chunk_id: Option<&'a RawValue>,
    #[serde(borrow)]
    exit_code: Option<&'a RawValue>,
}

fn text_failed(text: &str) -> bool {
    let text = text.trim_start();
    if !text.starts_with('{') {
        return text.lines().take_while(|line| !line.is_empty() && !line.starts_with("Output:"))
            .find_map(|line| line.strip_prefix("Process exited with code ")
                .and_then(|code| code.trim().parse::<i64>().ok()))
            .is_some_and(|code| code != 0);
    }
    let Ok(status) = serde_json::from_str::<OutputStatus>(text) else {
        return false;
    };
    let declared_error = status
        .metadata
        .and_then(|metadata| serde_json::from_str::<Value>(metadata.get()).ok())
        .is_some_and(|metadata| metadata["status"] == "error");
    declared_error
        || (status.chunk_id.is_some()
            && status
                .exit_code
                .and_then(|code| serde_json::from_str::<i64>(code.get()).ok())
                .is_some_and(|code| code != 0))
}

const TEXT_BLOCK_TYPES: [&str; 3] = ["input_text", "output_text", "text"];

pub(super) fn failed(output: &Value) -> bool {
    if let Some(parts) = output.as_array() {
        parts.iter().any(|part| {
            is_text(part["type"].as_str()) && part["text"].as_str().is_some_and(text_failed)
        })
    } else {
        output.as_str().is_some_and(text_failed)
    }
}

fn is_text(kind: Option<&str>) -> bool {
    kind.is_some_and(|kind| TEXT_BLOCK_TYPES.contains(&kind))
}

pub(super) fn raw_failed(output: &RawValue) -> bool {
    #[derive(Deserialize)]
    struct Part<'a> {
        #[serde(rename = "type")]
        kind: Option<String>,
        #[serde(borrow)]
        text: Option<&'a RawValue>,
    }
    let text_failed = |raw: &RawValue| {
        serde_json::from_str::<String>(raw.get())
            .ok()
            .is_some_and(|text| text_failed(&text))
    };
    // 折叠时只检查状态。数组中的非文本数据留到展开时读取。
    if output.get().starts_with('[') {
        serde_json::from_str::<Vec<&RawValue>>(output.get()).is_ok_and(|parts| {
            parts
                .into_iter()
                .filter_map(|part| serde_json::from_str::<Part<'_>>(part.get()).ok())
                .any(|part| is_text(part.kind.as_deref()) && part.text.is_some_and(text_failed))
        })
    } else {
        text_failed(output)
    }
}

pub(super) fn content(output: &Value) -> Value {
    if output.is_array() || output.is_object() {
        return output.clone();
    }
    let raw = output.as_str().unwrap_or("");
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Value::String(raw.to_owned());
    };
    let text = value["output"]
        .as_str()
        .or_else(|| value["content"].as_str())
        .map_or_else(|| value.to_string(), str::to_owned);
    Value::String(text)
}
