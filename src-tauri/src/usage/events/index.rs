//! 更新发生变化的来源及关联会话的请求索引。
use super::{RequestEvent, CACHE_WRITES, SERVICE_TIER};
use crate::usage::{cache::FileCache, normalize, request_parse::RequestOutcome, types::Sample};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub(in crate::usage) fn schema(db: &Connection) -> Result<(), String> {
    db.execute_batch(&format!("CREATE TABLE IF NOT EXISTS usage_request_index(
        agent TEXT PRIMARY KEY,revision INTEGER NOT NULL,indexed INTEGER NOT NULL);
        INSERT OR IGNORE INTO usage_request_index SELECT DISTINCT agent,1,0 FROM usage_ledger_sources;
        CREATE TABLE IF NOT EXISTS usage_request_events(
            id TEXT PRIMARY KEY,agent TEXT NOT NULL,time INTEGER NOT NULL,model TEXT NOT NULL,status TEXT NOT NULL,data TEXT NOT NULL,source INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS usage_requests_time ON usage_request_events(time DESC,id DESC);
        CREATE INDEX IF NOT EXISTS usage_requests_agent ON usage_request_events(agent,time DESC,id DESC);
        CREATE INDEX IF NOT EXISTS usage_requests_model ON usage_request_events(model,time DESC,id DESC);
        CREATE INDEX IF NOT EXISTS usage_requests_status ON usage_request_events(status,time DESC,id DESC);
        CREATE INDEX IF NOT EXISTS usage_requests_cache_writes ON usage_request_events(agent,time)
            WHERE {CACHE_WRITES};
        CREATE INDEX IF NOT EXISTS usage_requests_service_tier ON usage_request_events(agent,time)
            WHERE {SERVICE_TIER};"))
        .map_err(|_| "无法初始化请求记录索引，已保留用量账本".into())
}

pub(in crate::usage) fn from_sample(agent: &str, mut s: Sample) -> RequestEvent {
    let id = format!("{:x}", Sha256::digest(format!("{agent}:{}", s.key)));
    let status = s.request.status.unwrap_or_default();
    if s.request.scope.is_none() {
        s.request.scope = Some(
            if agent == "codex" && s.response_id.is_none() {
                "usage_delta"
            } else {
                "request"
            }
            .into(),
        );
    }
    let time = s.request.started_at.unwrap_or(s.timestamp);
    if s.request.scope.as_deref() == Some("usage_delta") {
        s.request.clear_timing();
    }
    let output_tps = s
        .request
        .latency_ms
        .filter(|ms| *ms > 0)
        .filter(|_| {
            status != RequestOutcome::Failed
                && s.details.output_recorded
                && !s.details.output_incomplete
                && s.request.scope.as_deref() == Some("request")
                && s.tokens.output_tokens > 0
        })
        .map(|ms| s.tokens.output_tokens as f64 * 1000.0 / ms as f64);
    RequestEvent {
        id,
        agent: agent.into(),
        time,
        session: s.session,
        model: s.model,
        status,
        partial: s.partial,
        tokens: s.tokens,
        details: s.details,
        request: s.request,
        output_tps,
        cost: None,
        pricing_issue: None,
    }
}

pub(in crate::usage) fn refresh(
    db: &mut Connection,
    agent: &str,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    let dirty: bool = db
        .query_row(
            "SELECT revision != indexed FROM usage_request_index WHERE agent=?1",
            [agent],
            |r| r.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取请求记录版本：{error}"))?
        .unwrap_or(false);
    if !dirty {
        return Ok(());
    }
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("无法开始请求记录更新：{error}"))?;
    let (revision, indexed): (i64, i64) = tx
        .query_row(
            "SELECT revision,indexed FROM usage_request_index WHERE agent=?1",
            [agent],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|error| format!("无法读取请求记录版本：{error}"))?;
    if indexed == 0 {
        // 仅升级旧账本时全量建立身份索引。事务失败/取消时旧事件仍然完整可用。
        let mut statement = tx
            .prepare("SELECT path,data FROM usage_ledger_sources WHERE agent=?1")
            .map_err(|_| "无法读取请求记录来源")?;
        let rows = statement
            .query_map([agent], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    crate::usage::storage::decode::<FileCache>(r.get_ref(1)?)?,
                ))
            })
            .map_err(|_| "无法读取请求记录来源")?;
        for row in rows {
            if cancel() {
                return Err("请求记录读取已取消".into());
            }
            let (path, file) = row.map_err(|_| "无法读取请求记录来源")?;
            crate::usage::ledger::source_keys(&tx, agent, &PathBuf::from(path), &file)?;
        }
        tx.execute(
            "UPDATE usage_ledger_sources SET dirty=1 WHERE agent=?1",
            [agent],
        )
        .map_err(|_| "无法升级请求来源索引")?;
        tx.execute("DELETE FROM usage_request_events WHERE agent=?1", [agent])
            .map_err(|_| "无法升级请求记录索引")?;
    }
    loop {
        let seed: Option<String> = tx
            .query_row(
                "SELECT path FROM usage_ledger_sources WHERE agent=?1 AND dirty=1 LIMIT 1",
                [agent],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "无法读取变化来源")?;
        let Some(seed) = seed else { break };
        let mut files = BTreeMap::<PathBuf, FileCache>::new();
        // 共享请求 ID、分页或父子关系的来源一起归并；无关会话的 data 不会被读取。
        let mut statement = tx
            .prepare(
                "WITH RECURSIVE affected(id) AS (
            SELECT id FROM usage_ledger_sources WHERE agent=?1 AND path=?2 UNION
            SELECT b.source FROM affected a
            JOIN usage_source_keys k ON k.source=a.id
            JOIN usage_source_keys b ON b.key=k.key
            JOIN usage_ledger_sources related ON related.id=b.source AND related.agent=?1
        ) SELECT s.path,s.data FROM affected a JOIN usage_ledger_sources s
            ON s.id=a.id ORDER BY s.path",
            )
            .map_err(|_| "无法查询相关会话")?;
        let rows = statement
            .query_map(params![agent, seed], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    crate::usage::storage::decode::<FileCache>(r.get_ref(1)?)?,
                ))
            })
            .map_err(|_| "无法读取相关会话")?;
        for row in rows {
            if cancel() {
                return Err("请求记录读取已取消".into());
            }
            let (path, mut file) = row.map_err(|_| "无法读取相关会话")?;
            for sample in file.samples.iter_mut().chain(&mut file.failures) {
                if sample.request.source.is_none() {
                    sample.request.source = file
                        .origin_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned());
                }
            }
            files.insert(path.into(), file);
        }
        update_sources(&tx, agent, &files, cancel)?;
    }
    tx.execute(
        "UPDATE usage_request_index SET indexed=?2 WHERE agent=?1",
        params![agent, revision],
    )
    .map_err(|_| "无法保存请求记录版本")?;
    tx.commit().map_err(|_| "无法提交请求索引".into())
}

fn update_sources(
    tx: &Connection,
    agent: &str,
    files: &BTreeMap<PathBuf, FileCache>,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    let samples = normalize::records(agent, files);
    let samples = normalize::unique(
        agent,
        files
            .values()
            .flat_map(|f| f.failures.iter().cloned())
            .chain(samples),
    );
    let owner = files.keys().next().ok_or("请求来源为空")?.to_string_lossy();
    let owner_id: i64 = tx
        .query_row(
            "SELECT id FROM usage_ledger_sources WHERE agent=?1 AND path=?2",
            params![agent, owner],
            |r| r.get(0),
        )
        .map_err(|_| "无法定位请求来源")?;
    let mut obsolete = BTreeSet::new();
    for path in files.keys() {
        let mut stmt = tx
            .prepare_cached("SELECT id FROM usage_request_events WHERE agent=?1 AND source=(SELECT id FROM usage_ledger_sources WHERE agent=?1 AND path=?2)")
            .map_err(|_| "无法读取原请求记录")?;
        let ids = stmt
            .query_map(params![agent, path.to_string_lossy()], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|_| "无法更新请求记录索引")?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|_| "无法更新请求记录索引")?;
        obsolete.extend(ids);
        tx.execute(
            "UPDATE usage_ledger_sources SET dirty=0 WHERE agent=?1 AND path=?2",
            params![agent, path.to_string_lossy()],
        )
        .map_err(|_| "无法完成请求来源更新")?;
    }
    {
        let mut insert = tx.prepare("INSERT INTO usage_request_events VALUES (?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(id) DO UPDATE SET time=excluded.time,model=excluded.model,status=excluded.status,data=excluded.data,source=excluded.source
            WHERE usage_request_events.data != excluded.data OR usage_request_events.source != excluded.source").map_err(|_| "无法保存请求记录")?;
        for sample in samples.into_iter().filter(|s| {
            s.model != "<synthetic>"
                && !(s.request.scope.as_deref() == Some("usage_delta") && s.tokens.total() == 0)
        }) {
            if cancel() {
                return Err("请求记录读取已取消".into());
            }
            let event = from_sample(agent, sample);
            let data = serde_json::to_string(&event).map_err(|_| "无法编码请求记录")?;
            obsolete.remove(&event.id);
            insert
                .execute(params![
                    event.id,
                    agent,
                    event.time,
                    event.model,
                    event.status.as_str(),
                    data,
                    owner_id
                ])
                .map_err(|_| "无法保存请求记录")?;
        }
    }
    // 新发现的父历史或明确响应 ID 可能取代旧累计差额。仅清除被归并取代的索引行，
    // 来源删除不会走这条分支：持久账本始终保留先前采集的原始观测。
    for id in obsolete {
        tx.execute("DELETE FROM usage_request_events WHERE id=?1", [id])
            .map_err(|_| "无法归并请求记录")?;
    }
    Ok(())
}
