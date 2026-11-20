use crate::usage::{ledger, roots, source::sessions::SessionInventory, AGENTS};
use chrono::{Duration, Utc};
use rusqlite::{params, Connection, TransactionBehavior};
use serde::Deserialize;
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupQuery {
    pub agents: BTreeSet<String>,
    pub older_than_days: Option<u32>,
}

pub fn clear_orphans(query: CleanupQuery) -> Result<usize, String> {
    if query.agents.is_empty()
        || query.agents.iter().any(|agent| !AGENTS.contains(&agent.as_str()))
    {
        return Err("请选择要清理的 Agent".into());
    }
    if query.older_than_days == Some(0) {
        return Err("清理时间范围无效".into());
    }
    let path = ledger::path()?;
    let _writer = ledger::SyncLock::acquire(&path, &|| false)?;
    let mut ledger = ledger::Ledger::open(Some(&path))?;
    let cutoff = query
        .older_than_days
        .map(|days| Utc::now().timestamp_millis() - Duration::days(days.into()).num_milliseconds());
    clear_orphans_at(&mut ledger.db, &query.agents, cutoff, roots)
}

pub(in crate::usage) fn clear_orphans_at(
    db: &mut Connection,
    agents: &BTreeSet<String>,
    cutoff: Option<i64>,
    roots: impl Fn(&str) -> Result<Vec<PathBuf>, String>,
) -> Result<usize, String> {
    for agent in agents {
        super::refresh(db, agent, &|| false)?;
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(cleanup_error)?;
    let (from, through) = cutoff.map_or((i64::MIN, i64::MAX), |time| (1, time.saturating_sub(1)));
    let mut removed = 0;
    for agent in agents {
        let mut select = tx
            .prepare_cached(
                "SELECT id,json_extract(data,'$.session'),
                    json_extract(data,'$.request.parentSessionId'),json_extract(data,'$.request.source')
                 FROM usage_request_events WHERE agent=?1 AND time BETWEEN ?2 AND ?3",
            )
            .map_err(cleanup_error)?;
        let mut rows = select.query_map(params![agent, from, through], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        }).map_err(cleanup_error)?.peekable();
        if rows.peek().is_none() {
            continue;
        }
        let sessions = SessionInventory::read(agent, &roots(agent)?)
            .map_err(|error| format!("无法核对 {agent} 的会话：{error}"))?;
        let mut orphans = Vec::new();
        for row in rows {
            let (id, session, parent, source) = row.map_err(cleanup_error)?;
            if !session.is_empty()
                && !sessions.contains(&session, parent.as_deref(), source.as_deref())
            {
                orphans.push(id);
            }
        }
        if orphans.is_empty() {
            continue;
        }
        let mut delete = tx
            .prepare_cached("DELETE FROM usage_request_events WHERE id=?1")
            .map_err(cleanup_error)?;
        let mut record_deletion = tx
            .prepare_cached("INSERT INTO usage_removed_requests(id) VALUES (?1)")
            .map_err(cleanup_error)?;
        for id in orphans {
            record_deletion.execute([&id]).map_err(cleanup_error)?;
            removed += delete.execute([&id]).map_err(cleanup_error)?;
        }
        tx.execute(
            "INSERT INTO usage_request_index(agent,revision,indexed) VALUES (?1,1,1)
             ON CONFLICT(agent) DO UPDATE SET revision=revision+1,indexed=indexed+1",
            [agent],
        )
        .map_err(cleanup_error)?;
    }
    tx.commit().map_err(cleanup_error)?;
    Ok(removed)
}

fn cleanup_error(error: rusqlite::Error) -> String {
    format!("无法清理孤儿记录：{error}")
}
