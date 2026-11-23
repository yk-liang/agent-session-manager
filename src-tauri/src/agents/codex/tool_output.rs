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
    failed: bool,
    exit_code: Option<i64>,
    reason: Option<String>,
    empty_command_output: bool,
}

impl Status {
    fn merge(mut self, other: Self) -> Self {
        self.failed |= other.failed;
        self.exit_code = self.exit_code.or(other.exit_code);
        self.reason = self.reason.or(other.reason);
        self.empty_command_output = false;
        self
    }

    pub fn annotate(self, result: &mut Value, function_output: bool) {
        result["is_error"] = Value::Bool(self.failed);
        if let Some(reason) = self.reason {
            result["error_summary"] = Value::String(reason);
        }
        if let Some(exit) = self.exit_code.filter(|_| function_output) {
            result["exit_code"] = Value::from(exit);
            if self.empty_command_output {
                result["empty_command_output"] = Value::Bool(true);
            }
        }
    }
}

fn text_status(text: &str) -> Status {
    let text = text.trim_start();
    if text.starts_with("Script failed") {
        return failed_status(None, Some(text), false);
    }
    if !text.starts_with('{') {
        let exit_code = text.lines().take_while(|line| !line.is_empty() && !line.starts_with("Output:") && !line.starts_with("Final output:"))
            .find_map(|line| line.strip_prefix("Process exited with code ")
                .and_then(|code| code.trim().parse::<i64>().ok()))
            .filter(|code| *code != 0);
        let body = execution_output(text);
        return if exit_code.is_some() { failed_status(exit_code, Some(body), body.len() < text.len()) } else { Status::default() };
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
    if declared_error || exit_code.is_some() {
        let value: Value = serde_json::from_str(text).unwrap_or_default();
        failed_status(exit_code, value["output"].as_str().or_else(|| value["content"].as_str()), !declared_error)
    } else {
        Status::default()
    }
}

fn reason(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|line| !line.is_empty()).map(|line| line.chars().take(240).collect())
}

pub(super) fn error_reason(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => reason(execution_output(text)),
        Value::Array(parts) => parts.iter().filter_map(|part| part["text"].as_str()).find_map(reason),
        Value::Object(fields) => ["message", "error", "detail"].into_iter()
            .filter_map(|key| fields.get(key)?.as_str()).find_map(reason),
        _ => None,
    }
}

fn failed_status(exit_code: Option<i64>, output: Option<&str>, command_output_known: bool) -> Status {
    Status {
        failed: true, exit_code, reason: output.and_then(reason),
        empty_command_output: command_output_known && exit_code == Some(1) && output.is_some_and(|body| body.trim().is_empty()),
    }
}

/// 旧版命令输出的信封。只在头部确有记录元数据时剥离，避免改写命令自己的输出。
pub(super) fn execution_output(text: &str) -> &str {
    if !text.starts_with("Chunk ID: ") || !text.lines().nth(1).is_some_and(|line| line.starts_with("Wall time: ")) {
        return text;
    }
    for (offset, line) in text.split_inclusive('\n').scan(0, |offset, line| {
        *offset += line.len();
        Some((*offset, line.trim_end()))
    }).take(8) {
        if matches!(line, "Output:" | "Final output:") {
            return &text[offset..];
        }
    }
    text
}

const TEXT_BLOCK_TYPES: [&str; 3] = ["input_text", "output_text", "text"];

pub(super) fn status(output: &Value) -> Status {
    if let Some(parts) = output.as_array() {
        let mut status = parts.iter().filter(|part| is_text(part["type"].as_str()))
            .filter_map(|part| part["text"].as_str()).map(text_status).reduce(Status::merge).unwrap_or_default();
        status.empty_command_output &= parts.len() == 1;
        status
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
        let parts = serde_json::from_str::<Vec<&RawValue>>(output.get()).unwrap_or_default();
        let mut result = parts.iter()
            .filter_map(|part| serde_json::from_str::<Part<'_>>(part.get()).ok())
            .filter(|part| is_text(part.kind.as_deref())).filter_map(|part| part.text)
            .map(status).reduce(Status::merge).unwrap_or_default();
        result.empty_command_output &= parts.len() == 1;
        result
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
