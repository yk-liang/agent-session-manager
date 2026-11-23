//! v2 会话的指令基线。OpenCode 只保存各指令来源的取值（instruction_state → instruction_blob），
//! 内置提示词随版本编译不落盘；这里按官方初始渲染格式还原各来源，作为系统指令上下文展示。
//! 渲染格式参照 anomalyco/opencode packages/core/src/{instructions,skill,reference,codemode}。
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

const ORDER: [&str; 6] = [
    "core/date",
    "core/environment",
    "core/instructions",
    "core/reference-guidance",
    "core/skill-guidance",
    "core/codemode",
];

pub(super) struct Baseline {
    /// 基线生效的消息序号；压缩完成后基线随之后移。
    pub epoch: i64,
    pub text: String,
}

pub(super) fn baseline(conn: &Connection, session_id: &str) -> rusqlite::Result<Option<Baseline>> {
    let exists: bool = conn.query_row(
        "SELECT count(*) = 2 FROM main.sqlite_master WHERE type='table' AND name IN ('instruction_state','instruction_blob')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let Some((epoch, values)) = conn
        .query_row(
            "SELECT epoch_start, initial_values FROM main.instruction_state WHERE session_id=?1",
            [session_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let hashes: BTreeMap<String, String> = serde_json::from_str(&values).unwrap_or_default();
    let mut blob = conn.prepare("SELECT value FROM main.instruction_blob WHERE hash=?1")?;
    let mut sources = Vec::new();
    for (key, hash) in &hashes {
        let value: Option<String> = blob.query_row([hash], |row| row.get(0)).optional()?;
        if let Some(value) = value.and_then(|value| serde_json::from_str::<Value>(&value).ok()) {
            sources.push((key.as_str(), value));
        }
    }
    sources.sort_by_key(|(key, _)| ORDER.iter().position(|known| known == key).unwrap_or(ORDER.len()));
    let text = sources.iter().filter_map(|(key, value)| render(key, value)).collect::<Vec<_>>().join("\n\n");
    Ok((!text.trim().is_empty()).then_some(Baseline { epoch, text }))
}

fn render(key: &str, value: &Value) -> Option<String> {
    let text = |value: &Value, field: &str| value[field].as_str().unwrap_or_default().to_owned();
    let items = || value.as_array().into_iter().flatten();
    Some(match key {
        "core/date" => format!("Today's date: {}", value.as_str()?),
        "core/environment" => format!("Here is some useful information about the environment you are running in:\n{}", value.as_str()?),
        "core/instructions" => items()
            .map(|file| format!("Instructions from: {}\n{}", text(file, "path"), text(file, "content")))
            .collect::<Vec<_>>()
            .join("\n\n"),
        "core/skill-guidance" => {
            let mut lines = vec![
                "Skills provide specialized instructions and workflows for specific tasks.".to_owned(),
                "Use the skill tool to load a skill when a task matches its description.".to_owned(),
                "The user may also invoke a skill directly. When that happens, its instructions appear in the conversation as a <skill_content> block, the same shape the skill tool returns. A skill that is already present this way does not need to be invoked again.".to_owned(),
            ];
            if items().next().is_none() {
                lines.push("No skills are currently available.".to_owned());
            } else {
                lines.extend(tagged("available_skills", "skill", items(), &["id", "name", "description"]));
            }
            lines.join("\n")
        }
        "core/reference-guidance" => {
            let mut lines = vec!["Project references provide additional directories that can be accessed when relevant.".to_owned()];
            lines.extend(tagged("available_references", "reference", items(), &["name", "path", "description"]));
            lines.join("\n")
        }
        "core/codemode" => {
            let mut lines = vec!["# Code Mode tools (call only inside `execute`)".to_owned()];
            for namespace in value["namespaces"].as_array().into_iter().flatten() {
                let entries = namespace["entries"].as_array().map_or(&[][..], Vec::as_slice);
                let count = namespace["count"].as_u64().unwrap_or(entries.len() as u64);
                let description = namespace["description"].as_str().map(|text| format!(" // {text}")).unwrap_or_default();
                lines.push(format!("- {} ({count} tools){description}", text(namespace, "name")));
                lines.extend(entries.iter().map(|entry| text(entry, "line")));
            }
            lines.join("\n")
        }
        _ => match value {
            Value::String(text) => text.clone(),
            _ => serde_json::to_string_pretty(value).ok()?,
        },
    })
}

fn tagged<'a>(list: &str, item: &str, values: impl Iterator<Item = &'a Value>, fields: &[&str]) -> Vec<String> {
    let mut lines = vec![format!("<{list}>")];
    for value in values {
        lines.push(format!("  <{item}>"));
        for field in fields {
            if let Some(text) = value[*field].as_str() {
                lines.push(format!("    <{field}>{text}</{field}>"));
            }
        }
        lines.push(format!("  </{item}>"));
    }
    lines.push(format!("</{list}>"));
    lines
}
