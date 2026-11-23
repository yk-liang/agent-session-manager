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

pub(super) fn read_header(path: &PathBuf) -> Result<(Option<ParsedRollout>, Vec<String>), String> {
    static HEADERS: OnceLock<FileCache<(Option<ParsedRollout>, Vec<String>)>> = OnceLock::new();
    HEADERS.get_or_init(|| FileCache::new(4096)).load(
        "codex-header",
        std::slice::from_ref(path),
        || Ok(read_rollout(path, ReadMode::Header, ReadLimit::default(), None, &mut super::tools::Activity::default()).split()),
        |(header, warnings)| header.is_some() && warnings.is_empty(),
    )
}

pub(super) fn read_headers(
    paths: &[PathBuf],
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Page>, String> {
    let mut pages = read_all_headers(paths, warnings, cancelled)?;
    let by_path: HashMap<_, _> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.path.clone(), i))
        .collect();
    let mut compressed_copies = HashSet::new();
    for page in &pages {
        if page.path.extension().is_some_and(|ext| ext == "zst") {
            if let Some(&i) = by_path.get(&page.path.with_extension("")) {
                // 压缩替换短暂重叠时优先使用仍可追加的原文。
                if same_rollout_identity(&page.header, &pages[i].header) {
                    compressed_copies.insert(page.path.clone());
                } else {
                    warnings.push(format!("原文与压缩副本的会话身份不一致：{:?}", page.path));
                }
            }
        }
    }
    pages.retain(|page| !compressed_copies.contains(&page.path));
    Ok(pages)
}

pub(super) fn validate_delete_copies(pages: &[&Page], targets: &HashSet<&str>) -> Result<(), String> {
    let by_path: HashMap<_, _> = pages.iter().map(|page| (&page.path, &page.header)).collect();
    for page in pages.iter().filter(|page| targets.contains(page.header.session_id.as_str())) {
        let (plain, compressed) = if page.path.extension().is_some_and(|ext| ext == "zst") {
            (page.path.with_extension(""), page.path.clone())
        } else {
            (page.path.clone(), page.path.with_extension("jsonl.zst"))
        };
        for path in [plain, compressed] {
            if path == page.path || !super::super::present(&path)? { continue; }
            if !by_path.get(&path).is_some_and(|header| same_rollout_identity(&page.header, header)) {
                return Err(format!("Codex 日志和压缩副本的身份无法确认：{path:?}"));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_delete_references(pages: &[&Page], targets: &HashSet<&str>) -> Result<(), String> {
    let mut physical_ids = HashMap::<String, &str>::new();
    for page in pages {
        let id = page.header.session_id.as_str();
        if !targets.contains(id) && filename_id(&page.path).is_some_and(|id| targets.contains(id)) {
            return Err(format!("Codex 文件名与会话身份冲突：{:?}", page.path));
        }
        for physical in [storage_id(page), filename_id(&page.path).unwrap_or(id).to_owned()] {
            if let Some(previous) = physical_ids.insert(physical, id) {
                if id != previous && (targets.contains(id) || targets.contains(previous)) {
                    return Err("Codex 日志的物理标识重复，无法确认删除范围".into());
                }
            }
        }
    }
    for page in pages.iter().filter(|page| !targets.contains(page.header.session_id.as_str())) {
        let references = page.header.history_base.as_ref().is_some_and(|base| {
            physical_ids.get(&base.thread_id).is_some_and(|id| targets.contains(id))
        });
        let attached = page.header.attached_thread && page.header.forked_from.as_deref().is_some_and(|id| targets.contains(id));
        if references || attached {
            return Err(format!("Codex 会话 {} 仍引用待删会话的历史", page.header.session_id));
        }
    }
    Ok(())
}

pub(super) fn filename_id(path: &Path) -> Option<&str> {
    let filename = path.file_name()?.to_str()?;
    let stem = filename.strip_suffix(".jsonl.zst").or_else(|| filename.strip_suffix(".jsonl"))?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    id.bytes().enumerate().all(|(i, byte)| {
        if matches!(i, 8 | 13 | 18 | 23) { byte == b'-' } else { byte.is_ascii_hexdigit() }
    }).then_some(id)
}

pub(super) fn read_all_headers(
    paths: &[PathBuf],
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Page>, String> {
    scan_headers(paths, cancelled, |_, diagnostics| warnings.extend(diagnostics))
}

pub(super) fn scan_headers(
    paths: &[PathBuf],
    cancelled: &dyn Fn() -> bool,
    mut report: impl FnMut(&Path, Vec<String>),
) -> Result<Vec<Page>, String> {
    let mut pages = Vec::new();
    for path in paths {
        check_catalog_cancelled(cancelled)?;
        let (header, ws) = read_header(path)?;
        if !ws.is_empty() {
            report(path, ws);
        }
        if let Some(header) = header {
            pages.push(Page {
                path: path.clone(),
                header,
            });
        }
    }
    pages.sort_by(|a, b| {
        super::super::timestamp(a.header.created_ts.as_deref())
            .cmp(&super::super::timestamp(b.header.created_ts.as_deref()))
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
    meta.size_bytes = Some(pages.iter().map(|p| p.header.size_bytes).sum());
    meta.source_files = pages.iter().map(|p| p.path.clone()).collect();
    let mut events = Vec::new();
    let mut inherited = 0;
    let mut has_activity = false;
    let mut dedup = EventDedup::default();
    let mut activity = super::tools::Activity::default();
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
            &mut activity,
        )
        .split();
        warnings.extend(ws);
        let Some(mut p) = parsed else { continue };
        if !same_rollout_identity(&p, &page.header) {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        dedup.commit_page();
        let first = super::super::timestamp(p.first_ts.as_deref());
        let previous = super::super::timestamp(meta.first_ts.as_deref());
        if first.is_some() && (previous.is_none() || first < previous) {
            meta.first_ts = p.first_ts;
        }
        if super::super::timestamp(p.last_ts.as_deref()) > super::super::timestamp(meta.last_ts.as_deref()) {
            meta.last_ts = p.last_ts;
        }
        if meta.title.is_none() {
            meta.title = p.first_user_text;
        }
        // 只沿有效分页前缀推进模型；新页未重复写模型时继续保留父历史的已知值。
        if let Some(model) = &meta.model {
            for event in &mut p.events {
                if event["type"] == "assistant" && event["message"]["model"].is_null() {
                    event["message"]["model"] = serde_json::json!(model);
                }
            }
        }
        if p.model.is_some() {
            meta.model = p.model;
        }
        meta.user_count += p.user_count;
        meta.assistant_count += p.assistant_count;
        meta.real_user_count += p.real_user_count;
        meta.assistant_text_count += p.assistant_text_count;
        has_activity |= p.has_activity;
        if events.is_empty() {
            inherited = p.inherited;
        }
        events.extend(p.events);
    }
    let thread = meta.codex.as_mut().unwrap();
    thread.empty = Some(!has_activity && meta.real_user_count == 0);
    activity.remove_redundant_scripts(&mut events, &mut inherited);
    Ok((
        SessionDetail {
            meta,
            events,
            inherited,
            warnings: Vec::new(),
        },
        warnings,
    ))
}

/// 子智能体分叉时 Codex 只复制精简后的父会话历史，执行记录等事件不会随副本保留。
/// 副本中的响应项沿用父会话的 ID：最后一个对应得上的位置之前改用父会话的完整记录，之后的副本原样保留。
pub(super) fn adopt_parent_history(detail: &mut SessionDetail, parent: Vec<Value>) {
    let item = |event: &Value| event["uuid"].as_str()?.split_once('@').map(|(id, _)| id.to_owned());
    let positions: HashMap<String, usize> = parent.iter().enumerate()
        .filter_map(|(index, event)| Some((item(event)?, index)))
        .collect();
    let Some((copy_end, parent_end)) = detail.events[..detail.inherited].iter().enumerate().rev()
        .find_map(|(index, event)| Some((index, *positions.get(&item(event)?)?)))
    else {
        return;
    };
    let mut events = parent;
    events.truncate(parent_end + 1);
    detail.inherited = events.len() + detail.inherited - copy_end - 1;
    events.extend(detail.events.drain(copy_end + 1..));
    detail.events = events;
}
