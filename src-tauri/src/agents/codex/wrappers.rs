//! 包装脚本的折叠：一段 `exec` 脚本只是转交一次工具调用，且同一轮里正好有这次调用的执行记录时，
//! 只保留执行记录，脚本的输出附在记录上。
use super::super::tool_ops::{text, ExecCall, Op};
use super::tools::{command_line, file_change_ops, image_path, parse_mcp_args, web_action};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum PatchKind {
    Add,
    Update,
    Delete,
}

/// 包装脚本与 `item_completed` 执行记录共同的身份：同一次命令、补丁、看图、MCP、网页动作或等待。
#[derive(PartialEq, Eq)]
enum ExecutionKey {
    Command { cmd: String, cwd: Option<String>, session_cwd: Option<String> },
    Patch(Vec<(String, PatchKind, Vec<(Option<String>, Vec<String>)>)>),
    Image(String),
    Mcp { server: String, tool: String, args: Value },
    WebSearch { queries: Vec<String>, domains: Vec<String>, recency: Option<u64> },
    WebOpen(String),
    WebFind { url: String, pattern: String },
    Sleep(Option<u64>),
}

fn hunk_matches((w_anchor, wrapper): &(Option<String>, Vec<String>), (e_anchor, execution): &(Option<String>, Vec<String>)) -> bool {
    if let Some(anchor) = w_anchor {
        let anchor_present = e_anchor.as_deref() == Some(anchor.as_str())
            || execution.iter().any(|line| line.strip_prefix(' ').is_some_and(|ctx| ctx.trim() == anchor));
        if !anchor_present {
            return false;
        }
    }
    if wrapper == execution {
        return true;
    }
    let has_context = |lines: &[String]| lines.iter().any(|line| line.starts_with(' '));
    wrapper.iter().filter(|line| line.starts_with(['+', '-']))
        .eq(execution.iter().filter(|line| line.starts_with(['+', '-'])))
        && has_context(wrapper)
        && has_context(execution)
        && (execution.windows(wrapper.len()).any(|slice| slice == wrapper.as_slice())
            || wrapper.windows(execution.len()).any(|slice| slice == execution.as_slice()))
}

impl ExecutionKey {
    fn matches(&self, execution: &Self) -> bool {
        match (self, execution) {
            (
                Self::Command { cmd: a, cwd: w_cwd, session_cwd },
                Self::Command { cmd: b, cwd: e_cwd, .. },
            ) => {
                a == b && match (w_cwd, e_cwd) {
                    (Some(w), e) => e.as_deref() == Some(w.as_str()),
                    (None, Some(e)) => session_cwd.as_deref() == Some(e.as_str()),
                    (None, None) => true,
                }
            }
            (Self::Patch(a), Self::Patch(b)) => a.len() == b.len() && a.iter().zip(b).all(|((pa, ka, ha), (pb, kb, hb))| {
                pa == pb && ka == kb && ha.len() == hb.len() && ha.iter().zip(hb).all(|(wa, eb)| hunk_matches(wa, eb))
            }),
            (Self::Sleep(a), Self::Sleep(b)) => a.is_none() || b.is_none() || a == b,
            _ => self == execution,
        }
    }
}

#[derive(Default)]
pub(super) struct Activity {
    pub calls: HashSet<String>,
    scripts: HashMap<String, ExecSpan>,
    ambiguous: HashSet<String>,
    executions: HashMap<String, BTreeMap<i64, Vec<(String, Option<ExecutionKey>)>>>,
}

struct ExecSpan {
    turn: String,
    start: i64,
    end: Option<i64>,
    succeeded: bool,
    execution: Option<ExecutionKey>,
}

impl Activity {
    pub fn start_exec(&mut self, id: &str, turn: String, start: i64, script: &str, session_cwd: Option<&str>) {
        if self.scripts.contains_key(id) {
            self.ambiguous.insert(id.into());
            return;
        }
        let execution = super::exec::wrapper(script).and_then(|call| wrapper_key(call, session_cwd));
        self.scripts.insert(id.into(), ExecSpan { turn, start, end: None, succeeded: false, execution });
    }

    pub fn finish_exec(&mut self, id: &str, end: i64, output: &serde_json::value::RawValue) {
        if self.ambiguous.contains(id) {
            return;
        }
        if let Some(span) = self.scripts.get_mut(id) {
            span.end = Some(end);
            span.succeeded = end >= span.start && super::tool_output::script_completed(output);
        }
    }

    pub fn record_execution(&mut self, turn: String, start: i64, item: &Value) {
        if let Some(id) = item["id"].as_str() {
            self.executions.entry(turn).or_default().entry(start).or_default().push((id.into(), execution_key(item)));
        }
    }

    /// `prefix` 是开头继承自父会话的事件数，随前缀内被折叠的事件一起减少。
    pub fn remove_redundant_scripts(&self, events: &mut Vec<Value>, prefix: &mut usize) {
        let boundary = *prefix;
        let mut uses: HashMap<&str, (usize, usize)> = HashMap::new();
        let mut results: HashMap<&str, (usize, usize)> = HashMap::new();
        for (pos, event) in events.iter().enumerate() {
            let Some(block) = event.pointer("/message/content/0") else { continue };
            match (block["type"].as_str(), block["id"].as_str(), block["tool_use_id"].as_str()) {
                (Some("tool_use"), Some(id), _) => {
                    let entry = uses.entry(id).or_insert((0, pos));
                    entry.0 += 1;
                }
                (Some("tool_result"), _, Some(id)) => {
                    let entry = results.entry(id).or_insert((0, pos));
                    entry.0 += 1;
                }
                _ => {}
            }
        }
        let mut by_turn: HashMap<&str, Vec<(&String, &ExecSpan)>> = HashMap::new();
        for (id, span) in &self.scripts {
            by_turn.entry(&span.turn).or_default().push((id, span));
        }
        let mut replaced: HashMap<&str, &str> = HashMap::new();
        for (turn, mut scripts) in by_turn {
            let Some(executions) = self.executions.get(turn) else { continue };
            scripts.sort_unstable_by_key(|(_, span)| span.start);
            let mut previous_end = None;
            for (index, (id, span)) in scripts.iter().enumerate() {
                let end = span.end.unwrap_or(i64::MAX).max(span.start);
                let overlaps = previous_end.is_some_and(|previous| previous >= span.start)
                    || scripts.get(index + 1).is_some_and(|(_, next)| next.start <= end);
                previous_end = Some(previous_end.map_or(end, |previous: i64| previous.max(end)));
                if span.succeeded && !overlaps && !self.ambiguous.contains(id.as_str()) {
                    let mut records = executions.range(span.start..=end).flat_map(|(_, records)| records);
                    if let (Some((execution, Some(key))), None) = (records.next(), records.next()) {
                        let same_occurrence = match (uses.get(id.as_str()), uses.get(execution.as_str()), results.get(id.as_str())) {
                            (Some(&(1, script_pos)), Some(&(1, exec_pos)), res) => {
                                (script_pos < boundary) == (exec_pos < boundary)
                                    && res.is_none_or(|&(count, res_pos)| count == 1 && (res_pos < boundary) == (script_pos < boundary))
                            }
                            _ => false,
                        };
                        if same_occurrence && span.execution.as_ref().is_some_and(|wrapper| wrapper.matches(key)) {
                            replaced.insert(id.as_str(), execution.as_str());
                        }
                    }
                }
            }
        }
        if replaced.is_empty() {
            return;
        }
        let mut outputs = HashMap::new();
        let mut position = 0;
        events.retain_mut(|event| {
            let inherited = position < boundary;
            position += 1;
            let Some(block) = event.pointer_mut("/message/content/0") else { return true };
            let id = match block["type"].as_str() {
                Some("tool_use") => &block["id"],
                Some("tool_result") => &block["tool_use_id"],
                _ => return true,
            };
            let Some(&execution) = id.as_str().and_then(|id| replaced.get(id)) else { return true };
            if block["type"] == "tool_result" {
                outputs.insert(execution, block.take());
            }
            *prefix -= usize::from(inherited);
            false
        });
        for event in events {
            let Some(block) = event.pointer_mut("/message/content/0") else { continue };
            if block["type"] == "tool_use" {
                if let Some(output) = block["id"].as_str().and_then(|id| outputs.remove(id)) {
                    block["script_result"] = output;
                }
            }
        }
    }
}

fn patch_anchors(patch: &str) -> HashMap<String, Vec<Option<String>>> {
    let mut anchors: HashMap<String, Vec<Option<String>>> = HashMap::new();
    let Some(body) = patch.trim_end().strip_prefix("*** Begin Patch\n").and_then(|body| body.strip_suffix("*** End Patch")) else {
        return anchors;
    };
    let mut current: Option<String> = None;
    for line in body.lines() {
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            current = Some(path.trim().into());
        } else if line.starts_with("*** Add File: ") || line.starts_with("*** Delete File: ") {
            current = None;
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if let Some(prev) = current.replace(path.trim().into()) {
                if let Some(list) = anchors.remove(&prev) {
                    anchors.insert(path.trim().into(), list);
                }
            }
        } else if let Some(path) = &current {
            if let Some(rest) = line.strip_prefix("@@") {
                let anchor = rest.trim();
                anchors.entry(path.clone()).or_default().push((!anchor.is_empty()).then(|| anchor.into()));
            } else if !anchors.contains_key(path) {
                anchors.entry(path.clone()).or_default().push(None);
            }
        }
    }
    anchors
}

fn unified_anchors(changes: &Value) -> HashMap<String, Vec<Option<String>>> {
    let mut anchors = HashMap::new();
    for (path, change) in changes.as_object().into_iter().flatten() {
        if matches!(change["type"].as_str(), Some("add" | "delete")) {
            continue;
        }
        let path = text(change, "move_path").unwrap_or(path);
        let list: Vec<_> = change["unified_diff"].as_str().unwrap_or_default().lines()
            .filter_map(|line| line.strip_prefix("@@ "))
            .map(|rest| rest.split_once(" @@").map(|(_, tail)| tail.trim()).filter(|tail| !tail.is_empty()).map(Into::into))
            .collect();
        anchors.insert(path.into(), list);
    }
    anchors
}

fn patch_signature(ops: &[Op], anchors: &HashMap<String, Vec<Option<String>>>) -> Option<Vec<(String, PatchKind, Vec<(Option<String>, Vec<String>)>)>> {
    let mut entries: Vec<_> = ops.iter().filter_map(|op| match op {
        Op::Write { path, content, .. } => {
            let lines = content.as_deref().unwrap_or_default().lines().map(|line| format!("+{line}")).collect();
            Some((path.clone(), PatchKind::Add, vec![(None, lines)]))
        }
        Op::Edit { path, hunks, .. } => {
            let file_anchors = anchors.get(path);
            let hunks = hunks.as_deref().unwrap_or_default().iter().enumerate()
                .map(|(index, hunk)| (file_anchors.and_then(|list| list.get(index)).cloned().flatten(), hunk.lines.clone()))
                .collect();
            Some((path.clone(), PatchKind::Update, hunks))
        }
        Op::Delete { path } => Some((path.clone(), PatchKind::Delete, Vec::new())),
        _ => None,
    }).collect();
    entries.sort_unstable();
    entries.dedup();
    (!entries.is_empty()).then_some(entries)
}

fn web_filters(entry: &Value) -> (Vec<String>, Option<u64>) {
    let mut domains: Vec<String> = entry["domains"].as_array().into_iter().flatten()
        .filter_map(Value::as_str).map(|d| d.trim().to_ascii_lowercase()).filter(|d| !d.is_empty()).collect();
    if let Some(domain) = text(entry, "domain") {
        domains.push(domain.trim().to_ascii_lowercase());
    }
    domains.sort_unstable();
    domains.dedup();
    (domains, entry["recency"].as_u64())
}

fn web_key(op: &Op, filter_source: &Value) -> Option<ExecutionKey> {
    match op {
        Op::WebSearch { queries, .. } if !queries.is_empty() => {
            let (domains, recency) = web_filters(filter_source);
            Some(ExecutionKey::WebSearch { queries: queries.clone(), domains, recency })
        }
        Op::WebFetch { url, prompt: None, .. } if !url.is_empty() => Some(ExecutionKey::WebOpen(url.clone())),
        Op::WebFetch { url, prompt: Some(pattern), find: true } if !url.is_empty() => {
            Some(ExecutionKey::WebFind { url: url.clone(), pattern: pattern.clone() })
        }
        _ => None,
    }
}

fn wrapper_key(call: ExecCall, session_cwd: Option<&str>) -> Option<ExecutionKey> {
    Some(match call.tool.as_str() {
        "exec_command" => ExecutionKey::Command {
            cmd: text(&call.input, "cmd")?.into(),
            cwd: text(&call.input, "workdir").map(Into::into),
            session_cwd: session_cwd.map(Into::into),
        },
        "apply_patch" => {
            let anchors = call.input.as_str().or_else(|| text(&call.input, "input")).map(patch_anchors).unwrap_or_default();
            ExecutionKey::Patch(patch_signature(&call.ops, &anchors)?)
        }
        "view_image" => ExecutionKey::Image(image_path(text(&call.input, "path")?)),
        // 原生记录一次只有一个网页动作：脚本也只能有一个动作、一个条目，且解码出有效操作。
        "web__run" => {
            let [op] = call.ops.as_slice() else { return None };
            let actions = call.input.as_object()?;
            let [(action, Value::Array(entries))] = actions.iter().filter(|(key, _)| *key != "response_length").collect::<Vec<_>>()[..] else { return None };
            let [entry] = entries.as_slice() else { return None };
            if !matches!(action.as_str(), "search_query" | "open" | "find") {
                return None;
            }
            web_key(op, entry)?
        }
        "clock__sleep" => ExecutionKey::Sleep(
            call.input["duration_ms"].as_u64()
                .or_else(|| call.input["seconds"].as_f64().filter(|s| s.is_finite() && *s >= 0.0).map(|s| (s * 1000.0).round() as u64)),
        ),
        tool => {
            let (server, tool) = tool.strip_prefix("mcp__")?.split_once("__")?;
            ExecutionKey::Mcp { server: server.into(), tool: tool.into(), args: call.input }
        }
    })
}

fn execution_key(item: &Value) -> Option<ExecutionKey> {
    Some(match item["type"].as_str()? {
        "CommandExecution" => ExecutionKey::Command {
            cmd: command_line(&item["command"]),
            cwd: text(item, "cwd").map(Into::into),
            session_cwd: None,
        },
        "FileChange" => ExecutionKey::Patch(patch_signature(&file_change_ops(&item["changes"]), &unified_anchors(&item["changes"]))?),
        "ImageView" => ExecutionKey::Image(image_path(text(item, "path")?)),
        "McpToolCall" => ExecutionKey::Mcp {
            server: text(item, "server")?.into(),
            tool: text(item, "tool")?.into(),
            args: parse_mcp_args(&item["arguments"]),
        },
        "Extension" if item["kind"] == "clock.sleep" => ExecutionKey::Sleep(item["durationMs"].as_u64()),
        "Extension" if item["kind"] == "web.search" => web_key(&web_action(&item["action"], text(item, "query"), None).1?, &item["action"])?,
        _ => return None,
    })
}
