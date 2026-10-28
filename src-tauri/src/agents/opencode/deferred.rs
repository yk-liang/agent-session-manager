//! 工具卡片只传递引用，展开后只读查询对应 part，不复制全部工具输出到界面。
use super::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::MetadataExt;
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
struct Location {
    path: PathBuf,
    session: String,
    message: String,
    part: String,
    identity: String,
    file_identity: Option<(u64, u64)>,
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
pub(super) fn register(
    path: &Path,
    session: &str,
    message: &str,
    part: &str,
    identity: &str,
    data: &str,
) -> String {
    let hash: [u8; 32] = Sha256::digest(data.as_bytes()).into();
    let file_identity = path.metadata().ok().map(|m| (m.dev(), m.ino()));
    let key = format!(
        "opencode:{:x}",
        Sha256::digest(
            serde_json::to_vec(&(path, session, message, part, file_identity, hash))
                .unwrap_or_default()
        )
    );
    let mut state = locations().lock().unwrap_or_else(|e| e.into_inner());
    if !state.entries.contains_key(&key) {
        while state.order.len() >= 8192 {
            if let Some(old) = state.order.pop_front() {
                state.entries.remove(&old);
            }
        }
        state.order.push_back(key.clone());
        state.entries.insert(
            key.clone(),
            Location {
                path: path.into(),
                session: session.into(),
                message: message.into(),
                part: part.into(),
                identity: identity.into(),
                file_identity,
                hash,
            },
        );
    }
    key
}
pub(crate) fn read(token: &str) -> Result<Value, String> {
    let (key, side) = token.rsplit_once(':').ok_or("工具内容引用无效")?;
    if !matches!(side, "input" | "result") {
        return Err("工具内容引用无效".into());
    }
    let location = locations()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries
        .get(key)
        .cloned()
        .ok_or("工具内容引用已过期，请重新打开会话")?;
    if location.file_identity != location.path.metadata().ok().map(|m| (m.dev(), m.ino())) {
        return Err("OpenCode 会话库已变化，请重新打开会话".into());
    }
    let conn = open_db_scoped(&location.path, Some(&location.session))?;
    let data: String = conn
        .query_row(
            "SELECT p.data FROM part p WHERE p.id=?1 AND p.message_id=?2
        AND EXISTS (SELECT 1 FROM message m WHERE m.id=?2 AND m.session_id=?3)",
            rusqlite::params![location.part, location.message, location.session],
            |r| r.get(0),
        )
        .map_err(|_| "工具记录已删除或不可读，请重新打开会话".to_string())?;
    // 允许其他会话持续写入 WAL；仅在本条记录被更新时拒绝旧引用。
    if <[u8; 32]>::from(Sha256::digest(data.as_bytes())) != location.hash {
        return Err("工具记录已更新，请重新打开会话".into());
    }
    let part: Value = serde_json::from_str(&data).map_err(|e| format!("工具记录无法解析：{e}"))?;
    let (call, result) = super::events::tool_blocks(&part, &location.identity, true);
    if side == "input" {
        Ok(call)
    } else {
        result.ok_or("工具尚无结果".into())
    }
}
