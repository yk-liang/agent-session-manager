//! 同一逻辑会话的分页日志：解析历史引用与有效前缀，统一供列表、详情、搜索使用。
use super::*;
use crate::agents::{catalog_cache::FileCache, check_catalog_cancelled};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

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
        *count > self.seen.get(&hash).copied().unwrap_or(0)
    }

    pub fn commit_page(&mut self) {
        // 本页解压或边界校验失败时会被丢弃，不能提前吞掉后续页里重放的有效消息。
        for (hash, count) in self.current.drain() {
            let previous = self.seen.entry(hash).or_default();
            *previous = (*previous).max(count);
        }
    }
}

pub(super) fn read_headers(
    paths: &[PathBuf],
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Page>, String> {
    static HEADERS: OnceLock<FileCache<(Option<ParsedRollout>, Vec<String>)>> = OnceLock::new();
    let cache = HEADERS.get_or_init(|| FileCache::new(4096));
    let mut pages = Vec::new();
    for path in paths {
        check_catalog_cancelled(cancelled)?;
        let (header, ws) = cache.load(
            "codex-header",
            std::slice::from_ref(path),
            || Ok(read_rollout(path, ReadMode::Header, ReadLimit::default(), None).split()),
            |(header, warnings)| header.is_some() && warnings.is_empty(),
        )?;
        warnings.extend(ws);
        if let Some(header) = header {
            pages.push(Page {
                path: path.clone(),
                header,
            });
        }
    }
    let by_path: HashMap<_, _> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.path.clone(), i))
        .collect();
    let mut compressed_copies = HashSet::new();
    for page in &pages {
        if page.path.extension().is_some_and(|ext| ext == "zst") {
            if let Some(&i) = by_path.get(&page.path.with_extension("")) {
                // 压缩替换的短暂交叠期优先使用仍可追加的原文。删除另行枚举、复核所有副本。
                if same_rollout_identity(&page.header, &pages[i].header) {
                    compressed_copies.insert(page.path.clone());
                } else {
                    warnings.push(format!("原文与压缩副本的会话身份不一致：{:?}", page.path));
                }
            }
        }
    }
    pages.retain(|page| {
        is_relevant_session(&page.header) && !compressed_copies.contains(&page.path)
    });
    pages.sort_by(|a, b| {
        a.header
            .created_ts
            .cmp(&b.header.created_ts)
            .then(a.path.cmp(&b.path))
    });
    Ok(pages)
}

pub(super) fn group_pages(pages: Vec<Page>) -> BTreeMap<String, Vec<Page>> {
    let mut groups: BTreeMap<String, Vec<Page>> = BTreeMap::new();
    for page in pages {
        groups
            .entry(page.header.session_id.clone())
            .or_default()
            .push(page);
    }
    groups
}

fn storage_stem(page: &Page) -> &str {
    let name = page
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    name.strip_suffix(".jsonl.zst")
        .or_else(|| name.strip_suffix(".jsonl"))
        .unwrap_or(name)
}

fn storage_id(page: &Page) -> String {
    let stem = storage_stem(page);
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
            let page = &pages[current];
            let parent = page.header.forked_from.as_deref();
            // 分叉起点以本线程 ID 命名，但引用可能指向父线程后续的物理页 ID。
            // 只有这个起点允许外部引用；子线程后续页的缺页仍须告警并阻止部分删除。
            let fork_root = parent.is_some_and(|id| id != page.header.session_id)
                && storage_stem(page).ends_with(&format!("-{}", page.header.session_id));
            if parent != Some(base.thread_id.as_str()) && !fork_root {
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
        // 压缩页的文件头无法确定正文长度；完整解析或读取受限前缀时再复核该边界。
        if base
            .end_byte_offset
            .zip(pages[previous].header.decoded_size_bytes)
            .is_some_and(|(end, size)| end > size)
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
        dedup.commit_page();
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
