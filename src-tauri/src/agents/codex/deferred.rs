//! 展开工具卡片时才读取大块输入输出。令牌只引用已经读取过的日志行。
use super::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
struct Location {
    path: PathBuf,
    device: u64,
    inode: u64,
    offset: u64,
    len: usize,
    hash: [u8; 32],
}
#[derive(Default)]
struct Locations {
    entries: HashMap<String, Location>,
    order: VecDeque<String>,
}
fn locations() -> &'static Mutex<Locations> {
    static STORE: OnceLock<Mutex<Locations>> = OnceLock::new();
    STORE.get_or_init(Mutex::default)
}
pub(super) fn register(path: &Path, identity: (u64, u64), offset: u64, line: &[u8]) -> String {
    let hash: [u8; 32] = Sha256::digest(line).into();
    let key = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(path, identity, offset, hash)).unwrap_or_default())
    );
    let mut state = locations().lock().unwrap_or_else(|e| e.into_inner());
    if !state.entries.contains_key(&key) {
        // 有界引用表不持有正文；旧令牌淘汰后可重新打开会话生成。
        while state.order.len() >= 32768 {
            if let Some(old) = state.order.pop_front() {
                state.entries.remove(&old);
            }
        }
        state.order.push_back(key.clone());
        state.entries.insert(
            key.clone(),
            Location {
                path: path.into(),
                device: identity.0,
                inode: identity.1,
                offset,
                len: line.len(),
                hash,
            },
        );
    }
    key
}
pub(crate) fn read(token: &str) -> Result<Value, String> {
    let location = locations()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries
        .get(token)
        .cloned()
        .ok_or("内容引用已过期，请重新打开会话")?;
    let mut file =
        super::super::open_jsonl(&location.path).map_err(|e| format!("无法读取工具内容：{e}"))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    // 允许 CLI 继续追加；原位改写、替换和截断必须重新验证。
    if (meta.dev(), meta.ino()) != (location.device, location.inode)
        || location.offset.saturating_add(location.len as u64) > meta.len()
    {
        return Err("会话文件已变化，请重新打开会话".into());
    }
    file.seek(SeekFrom::Start(location.offset))
        .map_err(|e| e.to_string())?;
    let mut bytes = vec![0; location.len];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    if <[u8; 32]>::from(Sha256::digest(&bytes)) != location.hash {
        return Err("会话内容已变化，请重新打开会话".into());
    }
    let record: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let mut events = Vec::new();
    convert_response_item(&record["payload"], None, None, &mut events);
    events
        .first()
        .and_then(|e| e.pointer("/message/content/0"))
        .cloned()
        .ok_or("工具内容不可用".into())
}
