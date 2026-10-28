//! 对话正文搜索：仅索引用户/助手可见正文；缓存按源文件版本更新，不写额外的正文副本。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use super::{AgentSource, Catalog, SessionMeta};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub project_dir: String,
    pub session_id: String,
}

impl From<&SessionMeta> for SessionRef {
    fn from(meta: &SessionMeta) -> Self {
        Self {
            project_dir: meta.project_dir.clone(),
            session_id: meta.session_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    #[serde(flatten)]
    pub session: SessionRef,
    /// 与 read_session 的统一事件数组一致，uuid 用于记录变化后的再次定位。
    pub event_index: usize,
    pub event_id: Option<String>,
    pub role: String,
    pub snippet: String,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub skipped: usize,
    pub cancelled: bool,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Scope {
    agent: String,
    root: Option<PathBuf>,
}

impl Scope {
    fn of(source: &dyn AgentSource) -> Self {
        Self {
            agent: source.id().to_string(),
            root: source.data_dir(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct IndexKey {
    scope: Scope,
    session: SessionRef,
}

#[derive(PartialEq, Eq)]
struct FileRevision {
    path: PathBuf,
    state: Option<(u64, Option<SystemTime>, u64)>,
}

fn revision(meta: &SessionMeta) -> Vec<FileRevision> {
    meta.source_files
        .iter()
        .map(|path| FileRevision {
            path: path.clone(),
            state: path
                .metadata()
                .ok()
                .map(|m| (m.len(), m.modified().ok(), m.ino())),
        })
        .collect()
}

struct Message {
    event_index: usize,
    event_id: Option<String>,
    role: String,
    text: String,
    folded: String,
}

struct Document {
    messages: Vec<Message>,
    bytes: usize,
}
struct Cached {
    revision: Vec<FileRevision>,
    document: Arc<Document>,
    used: u64,
}

struct Index {
    entries: HashMap<IndexKey, Cached>,
    bytes: usize,
    clock: u64,
    limit: usize,
}

impl Index {
    fn insert(&mut self, key: IndexKey, revision: Vec<FileRevision>, document: Arc<Document>) {
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.document.bytes;
        }
        if document.bytes > self.limit {
            return;
        }
        while self.bytes + document.bytes > self.limit {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, value)| value.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.bytes -= self.entries.remove(&oldest).unwrap().document.bytes;
        }
        self.clock += 1;
        self.bytes += document.bytes;
        self.entries.insert(
            key,
            Cached {
                revision,
                document,
                used: self.clock,
            },
        );
    }
}

struct SearchEngine {
    catalogs: Mutex<HashMap<Scope, HashMap<SessionRef, Option<SessionMeta>>>>,
    index: Mutex<Index>,
    generation: AtomicU64,
}

impl SearchEngine {
    fn new(limit: usize) -> Self {
        Self {
            catalogs: Mutex::default(),
            index: Mutex::new(Index {
                entries: HashMap::new(),
                bytes: 0,
                clock: 0,
                limit,
            }),
            generation: AtomicU64::new(0),
        }
    }

    fn start(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }
    fn current(&self, ticket: u64) -> bool {
        self.generation.load(Ordering::Acquire) == ticket
    }

    fn register(&self, source: &dyn AgentSource, catalog: &Catalog) {
        let scope = Scope::of(source);
        let mut sessions = HashMap::new();
        for meta in &catalog.sessions {
            // 同项目内重复 ID 无法唯一定位，不能从重复项中任意挑一个。
            sessions
                .entry(SessionRef::from(meta))
                .and_modify(|value| *value = None)
                .or_insert_with(|| Some(meta.clone()));
        }
        let mut index = self.index.lock().unwrap_or_else(|e| e.into_inner());
        index.entries.retain(|key, _| {
            key.scope != scope || sessions.get(&key.session).is_some_and(Option::is_some)
        });
        index.bytes = index
            .entries
            .values()
            .map(|entry| entry.document.bytes)
            .sum();
        drop(index);
        self.catalogs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(scope, sessions);
    }

    fn document(
        &self,
        source: &dyn AgentSource,
        key: IndexKey,
        meta: &SessionMeta,
        ticket: u64,
    ) -> Result<Arc<Document>, String> {
        let before = revision(meta);
        if !before.is_empty() {
            let mut index = self.index.lock().unwrap_or_else(|e| e.into_inner());
            index.clock += 1;
            let clock = index.clock;
            if let Some(cached) = index.entries.get_mut(&key) {
                if cached.revision == before {
                    cached.used = clock;
                    return Ok(cached.document.clone());
                }
            }
        }
        let events = source.search_events(meta)?;
        if !self.current(ticket) {
            return Err("Search cancelled".into());
        }
        let document = Arc::new(index_events(&events));
        // 读取过程中写入/替换了源文件时仍可返回本次读取内容，但不缓存这个不稳定版本。
        if !before.is_empty() && before == revision(meta) && self.current(ticket) {
            self.index.lock().unwrap_or_else(|e| e.into_inner()).insert(
                key,
                before,
                document.clone(),
            );
        }
        Ok(document)
    }

    fn search(
        &self,
        source: &dyn AgentSource,
        targets: &[SessionRef],
        query: &str,
        ticket: u64,
    ) -> Result<SearchResult, String> {
        let query = query.trim();
        if query.chars().count() > 256 {
            return Err("搜索词最多 256 个字符".into());
        }
        let mut result = SearchResult::default();
        if query.is_empty() {
            return Ok(result);
        }
        let needle = fold_case(query);
        let scope = Scope::of(source);
        let candidates: Vec<_> = {
            let catalogs = self.catalogs.lock().unwrap_or_else(|e| e.into_inner());
            let catalog = catalogs.get(&scope);
            let mut seen = HashSet::new();
            targets
                .iter()
                .filter(|key| seen.insert((*key).clone()))
                .map(|key| {
                    (
                        key.clone(),
                        catalog.and_then(|c| c.get(key)).and_then(Clone::clone),
                    )
                })
                .collect()
        };
        for (session, meta) in candidates {
            if !self.current(ticket) {
                result.cancelled = true;
                break;
            }
            let Some(meta) = meta else {
                result.skipped += 1;
                continue;
            };
            if !meta.has_transcript {
                continue;
            }
            let key = IndexKey {
                scope: scope.clone(),
                session: session.clone(),
            };
            match self.document(source, key, &meta, ticket) {
                Ok(document) => {
                    for message in &document.messages {
                        if let Some(at) = message.folded.find(&needle) {
                            let (start, end) = source_range(&message.text, at, at + needle.len());
                            result.hits.push(SearchHit {
                                session: session.clone(),
                                event_index: message.event_index,
                                event_id: message.event_id.clone(),
                                role: message.role.clone(),
                                snippet: snippet(&message.text, start, end),
                            });
                            break;
                        }
                    }
                }
                Err(_) => result.skipped += 1,
            }
        }
        if !self.current(ticket) {
            result.cancelled = true;
            result.hits.clear();
        }
        Ok(result)
    }
}

fn engine() -> &'static SearchEngine {
    static ENGINE: OnceLock<SearchEngine> = OnceLock::new();
    // 仅计算缓存的正文和大小写折叠字符串字节，较大的单会话按次读取，不挤占缓存。
    ENGINE.get_or_init(|| SearchEngine::new(64 * 1024 * 1024))
}

pub fn register(source: &dyn AgentSource, catalog: &Catalog) {
    engine().register(source, catalog);
}
pub fn start() -> u64 {
    engine().start()
}
pub fn cancel() {
    engine().start();
}
pub fn search(
    source: &dyn AgentSource,
    targets: &[SessionRef],
    query: &str,
    ticket: u64,
) -> Result<SearchResult, String> {
    engine().search(source, targets, query, ticket)
}

fn fold_case(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

fn index_events(events: &[Value]) -> Document {
    let mut messages = Vec::new();
    let mut bytes = 0;
    for (event_index, event) in events.iter().enumerate() {
        let Some(role @ ("user" | "assistant")) = event.get("type").and_then(Value::as_str) else {
            continue;
        };
        if role == "user" && event.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(content) = event.pointer("/message/content") else {
            continue;
        };
        let texts: Vec<&str> = match content {
            Value::String(text) => vec![text],
            Value::Array(parts) => parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect(),
            _ => Vec::new(),
        };
        for text in texts {
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            let folded = fold_case(&text);
            bytes += text.len() + folded.len();
            messages.push(Message {
                event_index,
                event_id: event
                    .get("uuid")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                role: role.into(),
                text,
                folded,
            });
        }
    }
    Document { messages, bytes }
}

/// 折叠后的 UTF-8 下标映射回原文，兼容大小写转换改变长度的字符及 emoji。
fn source_range(text: &str, start: usize, end: usize) -> (usize, usize) {
    let mut folded = 0;
    let mut original_start = None;
    for (offset, ch) in text.char_indices() {
        folded += ch.to_lowercase().map(char::len_utf8).sum::<usize>();
        if original_start.is_none() && start < folded {
            original_start = Some(offset);
        }
        if end <= folded {
            return (original_start.unwrap_or(offset), offset + ch.len_utf8());
        }
    }
    (original_start.unwrap_or(0), text.len())
}

fn snippet(text: &str, start: usize, end: usize) -> String {
    let begin = text[..start]
        .char_indices()
        .rev()
        .nth(12)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let finish = text[end..]
        .char_indices()
        .nth(60)
        .map(|(i, _)| end + i)
        .unwrap_or(text.len());
    format!(
        "{}{}{}",
        if begin > 0 { "…" } else { "" },
        &text[begin..finish],
        if finish < text.len() { "…" } else { "" }
    )
}

#[cfg(test)]
mod tests;
