//! 同一逻辑会话的分页日志：解析历史引用与有效前缀，统一供列表、详情、搜索使用。
use super::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Deserialize, PartialEq, Eq)]
pub(super) struct HistoryBase {
    thread_id: String,
    end_byte_offset: Option<u64>,
    end_ordinal_exclusive: Option<u64>,
}

pub(super) struct Page {
    pub path: PathBuf,
    pub header: ParsedRollout,
}

#[derive(Default)]
pub(super) struct EventDedup {
    seen: HashMap<[u8; 32], u32>,
    current: HashMap<[u8; 32], u32>,
}

impl EventDedup {
    pub fn begin_page(&mut self) {
        self.current.clear();
    }

    pub fn include(&mut self, record: &Value) -> bool {
        // 只消除跨文件的重放；同一页中同时间、同文本的多次真实消息仍按出现次数保留。
        let bytes = serde_json::to_vec(&(record.get("timestamp"), record.get("payload")))
            .unwrap_or_default();
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        let count = self.current.entry(hash).or_default();
        *count += 1;
        let previous = self.seen.entry(hash).or_default();
        if *count <= *previous {
            return false;
        }
        *previous = *count;
        true
    }
}

pub(super) fn headers(paths: Vec<PathBuf>, warnings: &mut Vec<String>) -> Vec<Page> {
    let mut pages = Vec::new();
    for path in paths {
        let (header, ws) =
            read_rollout(&path, ReadMode::Header, ReadLimit::default(), None).split();
        warnings.extend(ws);
        if let Some(header) = header.filter(is_relevant_session) {
            pages.push(Page { path, header });
        }
    }
    pages.sort_by(|a, b| {
        a.header
            .created_ts
            .cmp(&b.header.created_ts)
            .then(a.path.cmp(&b.path))
    });
    pages
}

pub(super) fn scan(root: &Path, warnings: &mut Vec<String>) -> BTreeMap<String, Vec<Page>> {
    let paths = all_rollouts_in(root, warnings);
    let mut groups: BTreeMap<String, Vec<Page>> = BTreeMap::new();
    for page in headers(paths, warnings) {
        groups
            .entry(page.header.session_id.clone())
            .or_default()
            .push(page);
    }
    groups
}

fn storage_id(page: &Page) -> String {
    let stem = page
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    // 文件名仅用来定位 history_base 引用的物理页；会话身份始终来自已解析的 session_meta。
    stem.rsplit_once('_')
        .map(|(_, id)| id.to_string())
        .unwrap_or_else(|| page.header.session_id.clone())
}

pub(super) fn resolve(pages: &[Page]) -> (Vec<(usize, ReadLimit)>, Vec<String>) {
    if pages.is_empty() {
        return (vec![], vec![]);
    }
    if !pages.iter().all(|p| p.header.paginated) {
        return (
            (0..pages.len())
                .map(|i| (i, ReadLimit::default()))
                .collect(),
            vec![],
        );
    }
    let mut by_id = HashMap::new();
    for (index, page) in pages.iter().enumerate() {
        if by_id.insert(storage_id(page), index).is_some() {
            return (
                vec![(pages.len() - 1, ReadLimit::default())],
                vec!["历史分页的物理标识重复，无法确定日志顺序".into()],
            );
        }
    }
    let mut current = pages.len() - 1;
    let mut limit = ReadLimit::default();
    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    let mut warnings = Vec::new();
    loop {
        if !visited.insert(current) {
            warnings.push("历史分页存在循环引用".into());
            break;
        }
        chain.push((current, limit));
        let Some(base) = pages[current].header.history_base.as_ref() else {
            break;
        };
        let Some(&previous) = by_id.get(&base.thread_id) else {
            // fork 的父会话拥有独立逻辑 ID，不把父线程混进当前会话或计作缺失分段。
            if pages[current].header.forked_from.as_deref() != Some(base.thread_id.as_str()) {
                warnings.push(format!(
                    "会话 {} 缺少历史分页 {}",
                    pages[current].header.session_id, base.thread_id
                ));
            }
            break;
        };
        if base.end_byte_offset.is_none() && base.end_ordinal_exclusive.is_none() {
            warnings.push("历史分页引用缺少有效读取边界".into());
            break;
        }
        if base
            .end_byte_offset
            .is_some_and(|end| end > pages[previous].header.size_bytes)
        {
            warnings.push("历史分页被截断，读取边界超出文件大小".into());
            break;
        }
        current = previous;
        limit = ReadLimit {
            bytes: base.end_byte_offset,
            ordinal: base.end_ordinal_exclusive,
        };
    }
    chain.reverse();
    (chain, warnings)
}

pub(super) fn read(pages: &[Page], mode: ReadMode) -> Result<(SessionDetail, Vec<String>), String> {
    let latest = pages.last().ok_or("缺少 Codex 会话日志")?;
    let (chain, mut warnings) = resolve(pages);
    // 目录统计可读取的本地片段；历史引用缺失在打开具体详情时提示，避免启动时反复告警。
    if mode == ReadMode::Summary {
        warnings.clear();
    }
    let mut meta = to_meta(&latest.header, None);
    meta.first_ts = None;
    meta.last_ts = None;
    meta.size_bytes = pages.iter().map(|p| p.header.size_bytes).sum();
    meta.source_files = pages.iter().map(|p| p.path.clone()).collect();
    let mut events = Vec::new();
    let mut dedup = EventDedup::default();
    let multiple_pages = chain.iter().filter(|(_, l)| l.bytes != Some(0)).count() > 1;
    for (index, limit) in chain {
        if limit.bytes == Some(0) {
            continue;
        }
        let page = &pages[index];
        dedup.begin_page();
        let (parsed, ws) = read_rollout(
            &page.path,
            mode,
            limit,
            if multiple_pages {
                Some(&mut dedup)
            } else {
                None
            },
        )
        .split();
        warnings.extend(ws);
        let Some(p) = parsed else { continue };
        if p.session_id != meta.session_id || !is_relevant_session(&p) {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        if p.first_ts.is_some() && (meta.first_ts.is_none() || p.first_ts < meta.first_ts) {
            meta.first_ts = p.first_ts;
        }
        if p.last_ts > meta.last_ts {
            meta.last_ts = p.last_ts;
        }
        if meta.title.is_none() {
            meta.title = p.first_user_text;
        }
        // 只沿有效分页前缀推进模型；新页未重复写模型时继续保留父历史的已知值。
        if p.model.is_some() {
            meta.model = p.model;
        }
        meta.user_count += p.user_count;
        meta.assistant_count += p.assistant_count;
        meta.real_user_count += p.real_user_count;
        meta.assistant_text_count += p.assistant_text_count;
        events.extend(p.events);
    }
    Ok((
        SessionDetail {
            meta,
            events,
            warnings: Vec::new(),
        },
        warnings,
    ))
}
