//! 编码与正式发布账本的原地升级。
use rusqlite::{types::ValueRef, Connection};
use serde::{de::DeserializeOwned, Serialize};

mod migration;

pub(super) use migration::released_cache;

pub(super) const VERSION: i64 = 7;

pub(super) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(value).map_err(|_| "无法编码用量记录")?;
    zstd::stream::encode_all(&*json, 3).map_err(|_| "无法压缩用量记录".into())
}

pub(super) fn decode<T: DeserializeOwned>(raw: ValueRef<'_>) -> rusqlite::Result<T> {
    let result = raw
        .as_blob()
        .map_err(|_| "用量记录必须是压缩数据")
        .and_then(|blob| zstd::stream::decode_all(blob).map_err(|_| "无法解压用量记录"))
        .and_then(|json| serde_json::from_slice(&json).map_err(|_| "无法解码用量记录"));
    result.map_err(|message| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            raw.data_type(),
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, message)),
        )
    })
}

const SOURCES: &str = "CREATE TABLE usage_ledger_sources(
    id INTEGER PRIMARY KEY,agent TEXT NOT NULL,path TEXT NOT NULL,data BLOB NOT NULL,
    dirty INTEGER NOT NULL DEFAULT 1,UNIQUE(agent,path));
    CREATE TABLE usage_source_keys(
    source INTEGER NOT NULL,key TEXT NOT NULL,PRIMARY KEY(source,key)) WITHOUT ROWID;
    CREATE TABLE usage_request_events(
    id TEXT PRIMARY KEY,agent TEXT NOT NULL,time INTEGER NOT NULL,model TEXT NOT NULL,
    status TEXT NOT NULL,data TEXT NOT NULL,source INTEGER NOT NULL DEFAULT 0);";

pub(super) type MigrationResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub(super) fn initialize(db: &rusqlite::Transaction<'_>, version: i64) -> Result<(), String> {
    (|| -> MigrationResult {
        match version {
            0 => {
                db.execute_batch(SOURCES)?;
                db.execute_batch("CREATE TABLE usage_ledger_updates(agent TEXT PRIMARY KEY,updated INTEGER NOT NULL);")?;
            }
            4 => migration::upgrade_v4(db)?,
            5 => migration::upgrade_v5(db)?,
            _ => return Err(format!("不支持的用量账本结构版本 {version}").into()),
        }
        finish_schema(db)?;
        db.pragma_update(None, "user_version", VERSION)?;
        Ok(())
    })()
    .map_err(|error| format!("无法初始化用量账本，已保留原数据：{error}"))
}

fn finish_schema(db: &Connection) -> MigrationResult {
    db.execute_batch(
        "CREATE INDEX IF NOT EXISTS usage_source_key_lookup ON usage_source_keys(key,source);
        CREATE INDEX IF NOT EXISTS usage_source_dirty ON usage_ledger_sources(agent,dirty,path);
        CREATE INDEX IF NOT EXISTS usage_request_source ON usage_request_events(agent,source);
        CREATE TABLE IF NOT EXISTS usage_sync_state(agent TEXT PRIMARY KEY,completed_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS usage_storage_state(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
        CREATE TABLE usage_ledger_stamps(
            agent TEXT NOT NULL,path TEXT NOT NULL,version INTEGER NOT NULL,stamps TEXT NOT NULL,
            PRIMARY KEY(agent,path)) WITHOUT ROWID;",
    )?;
    super::events::schema(db)?;
    Ok(())
}

pub(super) fn compact_migrated(db: &Connection) {
    let pending: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_storage_state WHERE key='compact_pending')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    if pending && db.execute_batch("VACUUM").is_ok() {
        let _ = db.execute("DELETE FROM usage_storage_state WHERE key='compact_pending'", []);
        let _ = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }
}
