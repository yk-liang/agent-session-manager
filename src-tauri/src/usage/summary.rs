//! 用量视图只聚合已持久化的请求事件。同步源文件与查询时间分桶是两个独立操作。
use super::{events, ingest, types::*, Store};
use chrono::{Local, TimeZone};
use rusqlite::{params_from_iter, Connection};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Health {
    pub partial: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SummaryQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    /// 来自界面本地日历的绝对时间边界，支持夏令时的 23/25 小时日及非整点时区。
    pub bounds: Vec<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRow {
    pub agent: String,
    pub bucket_start: Option<i64>,
    #[serde(flatten)]
    pub usage: UsageRow,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub rows: Vec<SummaryRow>,
    pub health: BTreeMap<String, Health>,
    pub revision: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Synced {
    pub updated_at: i64,
    pub checked_files: usize,
    pub changed_files: usize,
    pub bytes_read: u64,
}

pub fn sync(agent: &str, cancel: &dyn Fn() -> bool) -> Result<Synced, String> {
    let roots = super::roots(agent)?;
    let mut store = super::store(agent)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    sync_at(agent, &roots, &mut store, cancel)
}

pub(crate) fn sync_at(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    cancel: &dyn Fn() -> bool,
) -> Result<Synced, String> {
    // 即使稍后的文件不可读，也先把本轮已经入账的部分转成持久请求事件。
    let collected = ingest::collect(agent, roots, store, None, cancel);
    if !cancel() {
        events::refresh(&mut store.ledger()?.db, agent, cancel)?;
    }
    let result = collected?;
    Ok(Synced {
        updated_at: chrono::Utc::now().timestamp_millis(),
        checked_files: result.checked_files,
        changed_files: result.files,
        bytes_read: result.bytes_read,
    })
}

pub fn read(query: SummaryQuery, cancel: &dyn Fn() -> bool) -> Result<Summary, String> {
    let mut store = super::store("claude")
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut result = read_from(&mut store, query, cancel)?;
    drop(store);
    // 名称补全只看此次查询涉及的模型，不再次读取计量日志。
    for agent in ["qoder", "opencode"] {
        let ids = result
            .rows
            .iter()
            .filter(|r| r.agent == agent)
            .map(|r| r.usage.model.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if ids.is_empty() {
            continue;
        }
        let names = match agent {
            "qoder" => crate::agents::qoder::usage_model_names(&ids),
            _ => crate::agents::opencode::usage_model_names(&ids),
        };
        for row in result.rows.iter_mut().filter(|r| r.agent == agent) {
            if let Some(name) = names.get(&row.usage.model) {
                row.usage.model_name = name.clone();
            }
        }
    }
    Ok(result)
}

pub(crate) fn read_from(
    store: &mut Store,
    query: SummaryQuery,
    cancel: &dyn Fn() -> bool,
) -> Result<Summary, String> {
    read_at(&mut store.ledger()?.db, query, cancel)
}

pub(super) fn read_at(
    db: &mut Connection,
    query: SummaryQuery,
    cancel: &dyn Fn() -> bool,
) -> Result<Summary, String> {
    match (query.from, query.to) {
        (None, None) if query.bounds.is_empty() => {}
        (Some(from), Some(to))
            if from > 0
                && from < to
                && query.bounds.len() >= 2
                && query.bounds.len() <= 36626
                && query.bounds.first() == Some(&from)
                && query.bounds.last() == Some(&to)
                && query.bounds.windows(2).all(|w| w[0] < w[1]) => {}
        _ => return Err("用量时间分桶无效".into()),
    }
    for agent in super::AGENTS {
        if cancel() {
            return Err("用量查询已取消".into());
        }
        events::refresh(db, agent, cancel)?;
    }
    let tx = db.transaction().map_err(|_| "无法读取用量快照")?;
    let mut health = BTreeMap::<String, Health>::new();
    {
        let mut stmt = tx
            .prepare("SELECT agent,health FROM usage_ledger_sources WHERE health IS NOT NULL")
            .map_err(|_| "无法读取用量完整性")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|_| "无法读取用量完整性")?;
        for row in rows {
            let (agent, data) = row.map_err(|_| "无法读取用量完整性")?;
            let value: Health = serde_json::from_str(&data).map_err(|_| "用量完整性记录损坏")?;
            let entry = health.entry(agent).or_default();
            entry.partial |= value.partial;
            entry.warnings.extend(value.warnings);
        }
        for value in health.values_mut() {
            value.warnings.sort();
            value.warnings.dedup();
        }
    }
    let revision = {
        let mut stmt = tx
            .prepare("SELECT agent,indexed FROM usage_request_index ORDER BY agent")
            .map_err(|_| "无法读取用量版本")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{}:{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?
                ))
            })
            .map_err(|_| "无法读取用量版本")?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "无法读取用量版本")?
            .join("|")
    };
    let mut sql = "SELECT agent,time,model,data FROM usage_request_events".to_owned();
    let mut values = Vec::<rusqlite::types::Value>::new();
    if let Some((from, to)) = query.from.zip(query.to) {
        sql.push_str(" WHERE time>=? AND time<?");
        values.extend([from.into(), to.into()]);
    }
    let mut stmt = tx.prepare(&sql).map_err(|_| "无法查询请求用量")?;
    let mut rows = stmt
        .query(params_from_iter(values))
        .map_err(|_| "无法查询请求用量")?;
    let mut grouped =
        BTreeMap::<(String, String, String, Option<i64>), (Tokens, RecordedFields, bool)>::new();
    // SQL 先按事件时间范围筛选。只读账本中的小型计量记录，不读取或扫描会话正文。
    #[derive(Deserialize)]
    struct Measurement {
        #[serde(flatten)]
        tokens: Tokens,
        #[serde(flatten)]
        details: RecordedFields,
        partial: bool,
    }
    while let Some(row) = rows.next().map_err(|_| "无法读取请求计量")? {
        if cancel() {
            return Err("用量查询已取消".into());
        }
        let agent: String = row.get(0).map_err(|_| "请求来源无效")?;
        let time: i64 = row.get(1).map_err(|_| "请求时间无效")?;
        let model: String = row.get(2).map_err(|_| "请求模型无效")?;
        let data: String = row.get(3).map_err(|_| "请求计量无效")?;
        let m: Measurement =
            serde_json::from_str(&data).map_err(|_| "请求计量损坏，已保留原数据")?;
        // 没有 usage 的错误事件不变成计量为零的模型调用。
        if !m.details.input_recorded && !m.details.output_recorded && m.tokens.total() == 0 {
            continue;
        }
        let bucket_start = if query.bounds.is_empty() {
            None
        } else {
            Some(query.bounds[query.bounds.partition_point(|start| *start <= time) - 1])
        };
        let date = if bucket_start.is_some() {
            String::new()
        } else {
            Local
                .timestamp_millis_opt(time)
                .single()
                .filter(|_| time > 0)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        };
        if time <= 0 {
            let h = health.entry(agent.clone()).or_default();
            h.partial = true;
            h.warnings
                .push("部分请求缺少发生时间，只计入全部范围，不放入趋势".into());
        }
        let entry = grouped
            .entry((agent, model, date, bucket_start))
            .or_default();
        entry.0.add(&m.tokens);
        entry.1.merge(&m.details);
        entry.2 |= m.partial;
    }
    for h in health.values_mut() {
        h.warnings.sort();
        h.warnings.dedup();
    }
    let rows = grouped
        .into_iter()
        .map(
            |((agent, model, date, bucket_start), (tokens, details, partial))| SummaryRow {
                agent,
                bucket_start,
                usage: UsageRow {
                    date,
                    model_name: if model == "unknown" {
                        "未知模型".into()
                    } else {
                        model.clone()
                    },
                    model,
                    total_tokens: tokens.total(),
                    tokens,
                    details,
                    partial,
                },
            },
        )
        .collect();
    Ok(Summary {
        rows,
        health,
        revision,
    })
}
