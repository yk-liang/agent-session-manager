//! 存储编码和原地升级集中在此；解析、请求身份与去重算法不参与迁移。
use super::cache::FileCache;
use rusqlite::{types::ValueRef, Connection};
use serde::{de::DeserializeOwned, Serialize};

pub(super) const VERSION: i64 = 5;

pub(super) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(value).map_err(|_| "无法编码用量记录")?;
    compress(&json)
}

fn compress(json: &[u8]) -> Result<Vec<u8>, String> {
    zstd::stream::encode_all(json, 3).map_err(|_| "无法压缩用量记录".into())
}

/// TEXT 是旧版 JSON，BLOB 是完整 Zstandard 帧。保留双读，支持旧缓存及渐进补齐。
pub(super) fn decode<T: DeserializeOwned>(raw: ValueRef<'_>) -> rusqlite::Result<T> {
    let result = match raw {
        ValueRef::Text(json) => serde_json::from_slice(json).map_err(|_| "Invalid usage JSON"),
        ValueRef::Blob(blob) => zstd::stream::decode_all(blob)
            .map_err(|_| "Invalid compressed usage record")
            .and_then(|json| serde_json::from_slice(&json).map_err(|_| "Invalid usage JSON")),
        _ => Err("Invalid usage storage type"),
    };
    result.map_err(|message| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            raw.data_type(),
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                message,
            )),
        )
    })
}

/// 升级只改变字节表示，未知 JSON 字段也原样保留；损坏记录必须使整个事务回滚。
pub(super) fn compress_legacy(raw: ValueRef<'_>) -> Result<Vec<u8>, String> {
    let _: FileCache = decode(raw).map_err(|_| "用量记录损坏，已保留原文件")?;
    match raw {
        ValueRef::Text(json) => compress(json),
        ValueRef::Blob(blob) => Ok(blob.to_vec()),
        _ => unreachable!(),
    }
}

/// 调用方持有升级事务。编号只用于 SQLite 关联，原始来源键与请求身份保持不变。
pub(super) fn migrate(db: &Connection) -> Result<(), String> {
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        db.execute_batch(
            "CREATE TABLE usage_ledger_sources_v5(
            id INTEGER PRIMARY KEY,agent TEXT NOT NULL,path TEXT NOT NULL,data BLOB NOT NULL,
            dirty INTEGER NOT NULL DEFAULT 1,health TEXT,UNIQUE(agent,path));
            CREATE TABLE usage_source_keys_v5(
            source INTEGER NOT NULL,key TEXT NOT NULL,PRIMARY KEY(source,key)) WITHOUT ROWID;
            CREATE TABLE usage_request_events_v5(
            id TEXT PRIMARY KEY,agent TEXT NOT NULL,time INTEGER NOT NULL,model TEXT NOT NULL,
            status TEXT NOT NULL,data TEXT NOT NULL,source INTEGER NOT NULL DEFAULT 0);",
        )?;
        {
            let mut old = db.prepare("SELECT rowid,agent,path,data,dirty,health FROM usage_ledger_sources ORDER BY rowid")?;
            let mut rows = old.query([])?;
            let mut insert =
                db.prepare("INSERT INTO usage_ledger_sources_v5 VALUES (?1,?2,?3,?4,?5,?6)")?;
            while let Some(row) = rows.next()? {
                let bytes = compress_legacy(row.get_ref(3)?)?;
                insert.execute(rusqlite::params![
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    bytes,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<String>>(5)?
                ])?;
            }
        }
        db.execute_batch("INSERT INTO usage_source_keys_v5
            SELECT s.id,k.key FROM usage_source_keys k JOIN usage_ledger_sources_v5 s
            ON s.agent=k.agent AND s.path=k.path;
            INSERT INTO usage_request_events_v5
            SELECT e.id,e.agent,e.time,e.model,e.status,e.data,CASE WHEN e.source='' THEN 0 ELSE s.id END
            FROM usage_request_events e LEFT JOIN usage_ledger_sources_v5 s ON s.agent=e.agent AND s.path=e.source;")?;
        let same: bool = db.query_row("SELECT (SELECT count(*) FROM usage_source_keys)=(SELECT count(*) FROM usage_source_keys_v5)", [], |r| r.get(0))?;
        if !same {
            return Err("Usage identity relationships are incomplete".into());
        }
        db.execute_batch("DROP TABLE usage_source_keys; DROP TABLE usage_request_events; DROP TABLE usage_ledger_sources;
            ALTER TABLE usage_ledger_sources_v5 RENAME TO usage_ledger_sources;
            ALTER TABLE usage_source_keys_v5 RENAME TO usage_source_keys;
            ALTER TABLE usage_request_events_v5 RENAME TO usage_request_events;
            CREATE INDEX usage_source_key_lookup ON usage_source_keys(key,source);
            CREATE INDEX usage_source_dirty ON usage_ledger_sources(agent,dirty,path);
            CREATE INDEX usage_request_source ON usage_request_events(agent,source);
            CREATE TABLE IF NOT EXISTS usage_storage_state(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
            INSERT OR REPLACE INTO usage_storage_state VALUES ('compact_pending',1);")?;
        super::events::schema(db)?;
        db.pragma_update(None, "user_version", VERSION)?;
        Ok(())
    };
    run().map_err(|_| "无法升级用量存储，已保留原数据".into())
}

pub(super) fn compact_migrated(db: &Connection) {
    let pending: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_storage_state WHERE key='compact_pending')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(false);
    // VACUUM 自身原子提交。磁盘不足或其他写者忙时保留待处理标记，下次打开再试。
    // 不替换活跃数据库文件，已有 WAL 读快照继续由 SQLite 管理。
    if pending && db.execute_batch("VACUUM").is_ok() {
        let _ = db.execute(
            "DELETE FROM usage_storage_state WHERE key='compact_pending'",
            [],
        );
        let _ = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }
}
