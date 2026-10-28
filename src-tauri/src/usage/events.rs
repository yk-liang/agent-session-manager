//! 请求表只更新变化来源及共享身份/父历史的会话；翻页和汇总直接查询已存事件。
//! 旧账本升级才重建一次全部索引，源文件删除不会移除已采集的历史。
use super::{cache::FileCache, normalize, request_parse::RequestMeta, types::*};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventQuery {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub cursor: Option<EventCursor>,
    pub limit: Option<u32>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventCursor {
    pub time: i64,
    pub id: String,
    pub revision: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestEvent {
    pub id: String,
    pub agent: String,
    pub time: i64,
    pub session: String,
    pub model: String,
    pub model_name: String,
    pub status: String,
    pub partial: bool,
    #[serde(flatten)]
    pub tokens: Tokens,
    #[serde(flatten)]
    pub details: RecordedFields,
    pub request: RequestMeta,
    pub output_tps: Option<f64>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub models: Vec<String>,
    pub events: Vec<RequestEvent>,
    pub total: u64,
    pub next_cursor: Option<EventCursor>,
    pub reset: bool,
    pub revision: String,
}

pub(super) fn schema(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS usage_request_index(
        agent TEXT PRIMARY KEY,revision INTEGER NOT NULL,indexed INTEGER NOT NULL);
        INSERT OR IGNORE INTO usage_request_index SELECT DISTINCT agent,1,0 FROM usage_ledger_sources;
        CREATE TABLE IF NOT EXISTS usage_request_events(
            id TEXT PRIMARY KEY,agent TEXT NOT NULL,time INTEGER NOT NULL,model TEXT NOT NULL,status TEXT NOT NULL,data TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS usage_requests_time ON usage_request_events(time DESC,id DESC);
        CREATE INDEX IF NOT EXISTS usage_requests_agent ON usage_request_events(agent,time DESC,id DESC);
        CREATE INDEX IF NOT EXISTS usage_requests_model ON usage_request_events(model,time DESC,id DESC);
        CREATE INDEX IF NOT EXISTS usage_requests_status ON usage_request_events(status,time DESC,id DESC);")
        .map_err(|_| "无法初始化请求事件索引，已保留用量账本".into())
}

pub(super) fn from_sample(agent: &str, mut s: Sample) -> RequestEvent {
    s.migrate_availability();
    let id = format!("{:x}", Sha256::digest(format!("{agent}:{}", s.key)));
    let status = s.request.status.clone().unwrap_or_else(|| "unknown".into());
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
    let output_tps = s
        .request
        .latency_ms
        .filter(|ms| *ms > 0)
        .filter(|_| {
            status != "failed"
                && s.details.output_recorded
                && !s.details.output_incomplete
                && s.request.scope.as_deref() == Some("request")
                && s.tokens.output_tokens > 0
        })
        .map(|ms| s.tokens.output_tokens as f64 * 1000.0 / ms as f64);
    RequestEvent {
        id,
        agent: agent.into(),
        time: s.request.started_at.unwrap_or(s.timestamp),
        session: s.session,
        model_name: s.model.clone(),
        model: s.model,
        status,
        partial: s.partial,
        tokens: s.tokens,
        details: s.details,
        request: s.request,
        output_tps,
    }
}

pub(super) fn refresh(
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
        .map_err(|_| "无法读取请求事件版本")?
        .unwrap_or(false);
    if !dirty {
        return Ok(());
    }
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| "请求事件正在更新，请稍后重试")?;
    let (revision, indexed): (i64, i64) = tx
        .query_row(
            "SELECT revision,indexed FROM usage_request_index WHERE agent=?1",
            [agent],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "无法读取请求事件版本")?;
    if indexed == 0 {
        // 仅升级旧账本时全量建立身份索引。事务失败/取消时旧事件仍然完整可用。
        let mut statement = tx
            .prepare("SELECT path,data FROM usage_ledger_sources WHERE agent=?1")
            .map_err(|_| "无法读取请求事件来源")?;
        let rows = statement
            .query_map([agent], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|_| "无法读取请求事件来源")?;
        for row in rows {
            if cancel() {
                return Err("请求事件读取已取消".into());
            }
            let (path, data) = row.map_err(|_| "无法读取请求事件来源")?;
            let file: FileCache =
                serde_json::from_str(&data).map_err(|_| "用量账本记录损坏，已保留原文件")?;
            super::ledger::source_keys(&tx, agent, &PathBuf::from(path), &file)?;
        }
        tx.execute(
            "UPDATE usage_ledger_sources SET dirty=1 WHERE agent=?1",
            [agent],
        )
        .map_err(|_| "无法升级请求来源索引")?;
        tx.execute("DELETE FROM usage_request_events WHERE agent=?1", [agent])
            .map_err(|_| "无法升级请求事件索引")?;
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
                "WITH RECURSIVE affected(path) AS (
            SELECT ?2 UNION
            SELECT b.path FROM affected a
            JOIN usage_source_keys k ON k.agent=?1 AND k.path=a.path
            JOIN usage_source_keys b ON b.agent=?1 AND b.key=k.key
        ) SELECT s.path,s.data FROM affected a JOIN usage_ledger_sources s
            ON s.agent=?1 AND s.path=a.path ORDER BY s.path",
            )
            .map_err(|_| "无法查询相关会话")?;
        let rows = statement
            .query_map(params![agent, seed], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|_| "无法读取相关会话")?;
        for row in rows {
            if cancel() {
                return Err("请求事件读取已取消".into());
            }
            let (path, data) = row.map_err(|_| "无法读取相关会话")?;
            let mut file: FileCache =
                serde_json::from_str(&data).map_err(|_| "用量账本记录损坏，已保留原文件")?;
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
    .map_err(|_| "无法保存请求事件版本")?;
    tx.commit().map_err(|_| "请求事件索引未能落盘".into())
}

fn update_sources(
    tx: &Connection,
    agent: &str,
    files: &BTreeMap<PathBuf, FileCache>,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    let (samples, mut partial, mut warnings) = normalize::records(agent, &files);
    let known: BTreeSet<_> = samples.iter().map(|s| s.key.as_str()).collect();
    if files.values().any(|f| {
        f.state.missing_usage > 0
            || f.state
                .unmeasured_ids
                .iter()
                .any(|id| !known.contains(id.as_str()))
    }) {
        warnings.push("部分助手记录没有 usage，不能从文字推算消耗".into());
    }
    if files.values().any(|f| f.state.bad_lines > 0) {
        warnings.push("部分日志记录损坏或过大，已保留可读取计量".into());
    }
    if files.values().any(|f| f.pending) {
        warnings.push("未写完的日志尾行将在写入完成后补入".into());
    }
    warnings.extend(files.values().filter_map(|f| f.problem.clone()));
    warnings.sort();
    warnings.dedup();
    partial |= !warnings.is_empty();
    let health = serde_json::to_string(&super::summary::Health { partial, warnings })
        .map_err(|_| "无法保存计量完整性信息")?;
    let samples = normalize::unique(
        files
            .values()
            .flat_map(|f| f.failures.iter().cloned())
            .chain(samples),
    );
    let owner = files.keys().next().ok_or("请求来源为空")?.to_string_lossy();
    let mut obsolete = BTreeSet::new();
    for path in files.keys() {
        let mut stmt = tx
            .prepare_cached("SELECT id FROM usage_request_events WHERE agent=?1 AND source=?2")
            .map_err(|_| "无法读取原请求事件")?;
        let ids = stmt
            .query_map(params![agent, path.to_string_lossy()], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|_| "无法更新请求事件索引")?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|_| "无法更新请求事件索引")?;
        obsolete.extend(ids);
        tx.execute(
            "UPDATE usage_ledger_sources SET dirty=0,health=NULL WHERE agent=?1 AND path=?2",
            params![agent, path.to_string_lossy()],
        )
        .map_err(|_| "无法完成请求来源更新")?;
    }
    tx.execute(
        "UPDATE usage_ledger_sources SET health=?3 WHERE agent=?1 AND path=?2",
        params![agent, owner, health],
    )
    .map_err(|_| "无法保存计量完整性信息")?;
    {
        let mut insert = tx.prepare("INSERT INTO usage_request_events VALUES (?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(id) DO UPDATE SET time=excluded.time,model=excluded.model,status=excluded.status,data=excluded.data,source=excluded.source
            WHERE usage_request_events.data != excluded.data OR usage_request_events.source != excluded.source").map_err(|_| "无法保存请求事件")?;
        for sample in samples.into_iter().filter(|s| {
            s.model != "<synthetic>"
                && !(s.request.scope.as_deref() == Some("usage_delta") && s.tokens.total() == 0)
        }) {
            if cancel() {
                return Err("请求事件读取已取消".into());
            }
            let event = from_sample(agent, sample);
            let data = serde_json::to_string(&event).map_err(|_| "无法编码请求事件")?;
            obsolete.remove(&event.id);
            insert
                .execute(params![
                    event.id,
                    agent,
                    event.time,
                    event.model,
                    event.status,
                    data,
                    owner
                ])
                .map_err(|_| "无法保存请求事件")?;
        }
    }
    // 新发现的父历史或明确响应 ID 可能取代旧累计差额。仅清除被归并取代的索引行，
    // 来源删除不会走这条分支：持久账本始终保留先前采集的原始观测。
    for id in obsolete {
        tx.execute("DELETE FROM usage_request_events WHERE id=?1", [id])
            .map_err(|_| "无法归并请求事件")?;
    }
    Ok(())
}

pub fn read(query: EventQuery, cancel: &dyn Fn() -> bool) -> Result<EventPage, String> {
    let mut store = super::store("claude")
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    page(&mut store.ledger()?.db, query, cancel)
}

pub(super) fn page(
    db: &mut Connection,
    query: EventQuery,
    cancel: &dyn Fn() -> bool,
) -> Result<EventPage, String> {
    if query
        .agent
        .as_deref()
        .is_some_and(|a| !super::AGENTS.contains(&a))
    {
        return Err("不支持的请求事件来源".into());
    }
    if query
        .status
        .as_deref()
        .is_some_and(|s| !["success", "failed", "unknown"].contains(&s))
    {
        return Err("不支持的请求结果筛选".into());
    }
    if query.from.zip(query.to).is_some_and(|(a, b)| a >= b) {
        return Err("请求事件时间范围无效".into());
    }
    for agent in super::AGENTS {
        if cancel() {
            return Err("请求事件读取已取消".into());
        }
        if query.agent.as_deref().is_none_or(|a| a == agent) {
            refresh(db, agent, cancel)?;
        }
    }
    // count、页数据和版本来自同一个读快照；新写入只在下一次查询呈现。
    let tx = db.transaction().map_err(|_| "无法读取请求事件快照")?;
    let revision: String = {
        let mut stmt = tx
            .prepare("SELECT agent,indexed FROM usage_request_index ORDER BY agent")
            .map_err(|_| "无法读取请求事件版本")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{}:{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?
                ))
            })
            .map_err(|_| "无法读取请求事件版本")?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "无法读取请求事件版本")?
            .join("|")
    };
    let reset = query
        .cursor
        .as_ref()
        .is_some_and(|c| c.revision != revision);
    let models = {
        let mut statement = tx.prepare("SELECT DISTINCT model FROM usage_request_events
            WHERE (?1 IS NULL OR agent=?1) AND (?2 IS NULL OR time>=?2) AND (?3 IS NULL OR time<?3) ORDER BY model")
            .map_err(|_| "无法读取请求模型")?;
        let models = statement
            .query_map(params![query.agent, query.from, query.to], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|_| "无法读取请求模型")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "无法读取请求模型")?;
        models
    };
    let mut predicates = vec!["1=1".to_owned()];
    let mut values = Vec::<rusqlite::types::Value>::new();
    for (column, value) in [
        ("agent", query.agent),
        ("model", query.model),
        ("status", query.status),
    ] {
        if let Some(v) = value {
            predicates.push(format!("{column}=?"));
            values.push(v.into());
        }
    }
    if let Some(time) = query.from {
        predicates.push("time>=?".into());
        values.push(time.into());
    }
    if let Some(time) = query.to {
        predicates.push("time<?".into());
        values.push(time.into());
    }
    let filter = predicates.join(" AND ");
    let total: u64 = tx
        .query_row(
            &format!("SELECT count(*) FROM usage_request_events WHERE {filter}"),
            params_from_iter(&values),
            |r| r.get(0),
        )
        .map_err(|_| "无法读取请求事件数量")?;
    if let Some(c) = query.cursor.filter(|_| !reset) {
        predicates.push("(time<? OR (time=? AND id<?))".into());
        values.extend([c.time.into(), c.time.into(), c.id.into()]);
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 200) as usize;
    values.push(((limit + 1) as i64).into());
    let mut events: Vec<RequestEvent> = Vec::new();
    {
        let mut stmt = tx
            .prepare(&format!(
                "SELECT data FROM usage_request_events WHERE {} ORDER BY time DESC,id DESC LIMIT ?",
                predicates.join(" AND ")
            ))
            .map_err(|_| "无法读取请求事件")?;
        let rows = stmt
            .query_map(params_from_iter(&values), |r| r.get::<_, String>(0))
            .map_err(|_| "无法读取请求事件")?;
        for row in rows {
            if cancel() {
                return Err("请求事件读取已取消".into());
            }
            events.push(
                serde_json::from_str(&row.map_err(|_| "无法读取请求事件")?)
                    .map_err(|_| "请求事件索引损坏，请刷新重建")?,
            );
        }
    }
    let more = events.len() > limit;
    events.truncate(limit);
    let next_cursor = if more {
        events.last().map(|e| EventCursor {
            time: e.time,
            id: e.id.clone(),
            revision: revision.clone(),
        })
    } else {
        None
    };
    Ok(EventPage {
        events,
        total,
        next_cursor,
        reset,
        revision,
        models,
    })
}
