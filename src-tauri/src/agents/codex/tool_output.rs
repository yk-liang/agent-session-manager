//! 工具结果可保存为旧版字符串或 Responses 内容块数组。
use serde::Deserialize;
use serde_json::{value::RawValue, Value};

#[derive(Deserialize)]
struct OutputMetadata<'a> {
    #[serde(borrow)]
    metadata: Option<&'a RawValue>,
    #[serde(borrow)]
    chunk_id: Option<&'a RawValue>,
    #[serde(borrow)]
    exit_code: Option<&'a RawValue>,
}

#[derive(Default)]
pub(super) struct Status {
    pub failed: bool,
    pub exit_code: Option<i64>,
}

impl Status {
    fn merge(mut self, other: Self) -> Self {
        self.failed |= other.failed;
        self.exit_code = self.exit_code.or(other.exit_code);
        self
    }
}

fn text_status(text: &str) -> Status {
    let text = text.trim_start();
    if text.starts_with("Script failed") {
        return Status { failed: true, exit_code: None };
    }
    if !text.starts_with('{') {
        let exit_code = text.lines().take_while(|line| !line.is_empty() && !line.starts_with("Output:") && !line.starts_with("Final output:"))
            .find_map(|line| line.strip_prefix("Process exited with code ")
                .and_then(|code| code.trim().parse::<i64>().ok()))
            .filter(|code| *code != 0);
        return Status { failed: exit_code.is_some(), exit_code };
    }
    let Ok(status) = serde_json::from_str::<OutputMetadata>(text) else {
        return Status::default();
    };
    let declared_error = status
        .metadata
        .and_then(|metadata| serde_json::from_str::<Value>(metadata.get()).ok())
        .is_some_and(|metadata| metadata["status"] == "error");
    let exit_code = status.chunk_id.and(status.exit_code)
        .and_then(|code| serde_json::from_str::<i64>(code.get()).ok())
        .filter(|code| *code != 0);
    Status { failed: declared_error || exit_code.is_some(), exit_code }
}

const TEXT_BLOCK_TYPES: [&str; 3] = ["input_text", "output_text", "text"];

pub(super) fn status(output: &Value) -> Status {
    if let Some(parts) = output.as_array() {
        parts.iter().filter(|part| is_text(part["type"].as_str()))
            .filter_map(|part| part["text"].as_str()).map(text_status).fold(Status::default(), Status::merge)
    } else {
        output.as_str().map_or_else(Status::default, text_status)
    }
}

fn is_text(kind: Option<&str>) -> bool {
    kind.is_some_and(|kind| TEXT_BLOCK_TYPES.contains(&kind))
}

pub(super) fn script_completed(output: &RawValue) -> bool {
    #[derive(Deserialize)]
    struct Part {
        text: Option<String>,
    }
    let first = match serde_json::from_str::<Vec<&RawValue>>(output.get()) {
        Ok(parts) => parts.first().and_then(|part| serde_json::from_str::<Part>(part.get()).ok()).and_then(|part| part.text),
        Err(_) => serde_json::from_str::<String>(output.get()).ok(),
    };
    first.is_some_and(|text| text.starts_with("Script completed"))
}

pub(super) fn raw_status(output: &RawValue) -> Status {
    #[derive(Deserialize)]
    struct Part<'a> {
        #[serde(rename = "type")]
        kind: Option<String>,
        #[serde(borrow)]
        text: Option<&'a RawValue>,
    }
    let status = |raw: &RawValue| {
        serde_json::from_str::<String>(raw.get())
            .ok()
            .map_or_else(Status::default, |text| text_status(&text))
    };
    // 折叠时只检查状态。数组中的非文本数据留到展开时读取。
    if output.get().starts_with('[') {
        serde_json::from_str::<Vec<&RawValue>>(output.get()).unwrap_or_default().into_iter()
            .filter_map(|part| serde_json::from_str::<Part<'_>>(part.get()).ok())
            .filter(|part| is_text(part.kind.as_deref())).filter_map(|part| part.text)
            .map(status).fold(Status::default(), Status::merge)
    } else {
        status(output)
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
