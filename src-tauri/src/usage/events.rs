//! 从同一账本快照查询筛选项、时间分布和请求分页。
use super::{
    codex::LEGACY_ESTIMATE,
    request_parse::{RequestMeta, RequestOutcome},
    types::*,
};
use rusqlite::{params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventQuery {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub project: Option<String>,
    pub status: Option<String>,
    pub include_filters: bool,
    pub from: Option<i64>,
    pub to: Option<i64>,
    /// 从 1 开始的页码；越界时落到最后一页，空结果仍为第 1 页。
    pub page: Option<u64>,
    pub limit: Option<u32>,
    /// 仅自动刷新使用：返回包含当前可见请求的页，新增记录不会挤走阅读位置。
    pub anchor: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestEvent {
    pub id: String,
    pub agent: String,
    pub time: i64,
    pub session: String,
    pub model: String,
    pub status: RequestOutcome,
    pub partial: bool,
    #[serde(flatten)]
    pub tokens: Tokens,
    #[serde(flatten)]
    pub details: RecordedFields,
    pub request: RequestMeta,
    pub output_tps: Option<f64>,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub cost: Option<super::pricing::EventCost>,
    /// 查询时计算，cost 为空时说明原因；不写入账本。
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub pricing_issue: Option<super::pricing::PricingIssue>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub events: Vec<RequestEvent>,
    pub total: u64,
    pub has_cache_writes: bool,
    pub has_service_tiers: bool,
    pub page: u64,
    pub page_size: u32,
    pub revision: String,
    pub pricing_revision: u64,
    /// 与 total 同一快照、同一筛选条件的时间分布。
    pub timeline: Timeline,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<RequestFilters>,
}

#[derive(Debug, Default, Serialize)]
pub struct RequestFilters {
    pub models: BTreeMap<String, u64>,
    pub projects: BTreeMap<String, u64>,
    pub results: BTreeMap<String, u64>,
}

const PROJECT: &str = "coalesce(json_extract(data,'$.request.project'),'')";
const CACHE_WRITES: &str =
    "CASE WHEN json_valid(data) THEN json_extract(data,'$.cacheWriteTokens') > 0 END";
const SERVICE_TIER: &str =
    "CASE WHEN json_valid(data) THEN trim(json_extract(data,'$.request.tier')) <> '' END";
// 旧账本的 unknown 与 success 同属未检测到失败的记录。
pub(super) const IS_FAILED: &str = "status='failed'";

pub(super) const DISTINCT_REQUESTS: &str = "NOT (
    agent='opencode' AND time>0 AND json_extract(data,'$.request.nativeSession') IS NOT NULL
    AND EXISTS (
        SELECT 1 FROM usage_request_events AS native
        WHERE native.agent='claude'
          AND native.time>=usage_request_events.time/86400000*86400000
          AND native.time<usage_request_events.time/86400000*86400000+86400000
          AND json_extract(native.data,'$.session')=json_extract(usage_request_events.data,'$.request.nativeSession')
          AND json_extract(native.data,'$.request.kind') IS NOT 'sidechain'
          AND json_type(native.data,'$.inputTokens') IS NOT NULL
          AND (json_extract(native.data,'$.inputTokens')>0 OR json_extract(native.data,'$.outputTokens')>0)
    ))";

mod index;

#[cfg(test)]
pub(super) use index::from_sample;
pub(super) use index::{refresh, schema};

fn filter_counts(
    db: &Connection,
    scope: &str,
    values: &[rusqlite::types::Value],
    query: &EventQuery,
    cancel: &dyn Fn() -> bool,
) -> Result<RequestFilters, String> {
    let error = |_| "无法读取请求筛选项".to_owned();
    let mut stmt = db
        .prepare(&format!(
            "SELECT model,{PROJECT},{IS_FAILED} AS failed,count(*) FROM usage_request_events
         WHERE {scope} GROUP BY model,{PROJECT},failed"
        ))
        .map_err(error)?;
    let mut rows = stmt.query(params_from_iter(values)).map_err(error)?;
    let mut filters = RequestFilters::default();
    while let Some(row) = rows.next().map_err(error)? {
        if cancel() {
            return Err("请求记录读取已取消".into());
        }
        let model: String = row.get(0).map_err(error)?;
        let project: String = row.get(1).map_err(error)?;
        let status = if row.get::<_, bool>(2).map_err(error)? {
            RequestOutcome::Failed
        } else {
            RequestOutcome::Success
        }
        .as_str();
        let count = row.get::<_, i64>(3).map_err(error)? as u64;
        let is_model = query.model.as_ref().is_none_or(|value| *value == model);
        let is_project = query.project.as_ref().is_none_or(|value| *value == project);
        let is_status = query.status.as_deref().is_none_or(|value| value == status);
        if is_project && is_status {
            *filters.models.entry(model).or_default() += count;
        }
        if is_model && is_status {
            *filters.projects.entry(project).or_default() += count;
        }
        if is_model && is_project {
            *filters.results.entry(status.into()).or_default() += count;
        }
    }
    Ok(filters)
}

/// 筛选结果按本地时间对齐的等长时段计数（最多 TIMELINE_BUCKETS 段），只用索引列聚合，
/// 不读取请求 JSON。缺少发生时间的记录单列在 untimed，按时间倒序排在最后。
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    pub start: i64,
    pub step: i64,
    pub success: Vec<u64>,
    pub failed: Vec<u64>,
    pub untimed: StatusCounts,
}
#[derive(Debug, Default, Serialize)]
pub struct StatusCounts {
    pub success: u64,
    pub failed: u64,
}

const TIMELINE_BUCKETS: i64 = 120;
const MINUTE: i64 = 60_000;
const DAY: i64 = 1440 * MINUTE;

/// 选择不超过 TIMELINE_BUCKETS 段的最小整齐步长；超过一周时取整天倍数。
fn timeline_step(span: i64) -> i64 {
    [
        1, 2, 5, 10, 15, 30, 60, 120, 180, 360, 720, 1440, 2880, 10080,
    ]
    .into_iter()
    .map(|m| m * MINUTE)
    .find(|step| (span - 1) / step + 1 <= TIMELINE_BUCKETS)
    .unwrap_or_else(|| ((span - 1) / (TIMELINE_BUCKETS * DAY) + 1) * DAY)
}

fn local_midnight(ms: i64) -> i64 {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_millis_opt(ms)
        .single()
        .and_then(|t| t.date_naive().and_hms_opt(0, 0, 0))
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map_or(ms - ms.rem_euclid(DAY), |t| t.timestamp_millis())
}

fn timeline(
    tx: &Connection,
    filter: &str,
    values: &[rusqlite::types::Value],
    from: Option<i64>,
    to: Option<i64>,
    now: i64,
) -> Result<Timeline, String> {
    let error = |_| "无法统计请求时间分布".to_owned();
    let (oldest, newest): (Option<i64>, Option<i64>) = tx
        .query_row(
            &format!(
                "SELECT min(time),max(time) FROM usage_request_events WHERE {filter} AND time>0"
            ),
            params_from_iter(values),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(error)?;
    let end = to.unwrap_or_else(|| now.max(newest.unwrap_or(now))).max(1);
    let from = from.or(oldest).unwrap_or(end).clamp(1, end);
    let step = timeline_step((end - from).max(1));
    let mut start = local_midnight(from);
    if step < DAY {
        start += (from - start) / step * step;
    }
    let n = (end.saturating_sub(start).saturating_sub(1) / step + 1).max(1);
    let mut result = Timeline {
        start,
        step,
        success: vec![0; n as usize],
        failed: vec![0; n as usize],
        untimed: StatusCounts::default(),
    };
    // 超出范围的时间钳入首尾时段，使“某时段之后的记录数”与按时间倒序的分页偏移完全一致。
    let mut stmt = tx
        .prepare(&format!(
            "SELECT CASE WHEN time>0 THEN min(max((time-?)/?,0),?) END AS bucket,{IS_FAILED} AS failed,count(*)
             FROM usage_request_events WHERE {filter} GROUP BY bucket,failed"
        ))
        .map_err(error)?;
    // 参数顺序：先 SELECT 中的三个占位符，再 WHERE。
    let mut ordered: Vec<rusqlite::types::Value> = vec![start.into(), step.into(), (n - 1).into()];
    ordered.extend(values.iter().cloned());
    let mut rows = stmt.query(params_from_iter(ordered)).map_err(error)?;
    while let Some(row) = rows.next().map_err(error)? {
        let bucket: Option<i64> = row.get(0).map_err(error)?;
        let failed: bool = row.get(1).map_err(error)?;
        let count = u64::try_from(row.get::<_, i64>(2).map_err(error)?).unwrap_or(0);
        let count_for_outcome = match (bucket, failed) {
            (Some(i), true) => &mut result.failed[i as usize],
            (Some(i), false) => &mut result.success[i as usize],
            (None, true) => &mut result.untimed.failed,
            (None, false) => &mut result.untimed.success,
        };
        *count_for_outcome += count;
    }
    Ok(result)
}

impl Timeline {
    fn len(&self) -> u64 {
        self.success.iter().chain(&self.failed).sum::<u64>()
            + self.untimed.success
            + self.untimed.failed
    }
}

pub fn read(query: EventQuery, cancel: &dyn Fn() -> bool) -> Result<EventPage, String> {
    if cancel() {
        return Err("请求记录读取已取消".into());
    }
    let mut db = super::ledger::reader(&super::ledger::path()?)?;
    page(&mut db, query, cancel)
}

pub(super) fn page(
    db: &mut Connection,
    query: EventQuery,
    cancel: &dyn Fn() -> bool,
) -> Result<EventPage, String> {
    page_with_pricer(db, query, super::pricing::global(), cancel)
}

pub(super) fn page_with_pricer(
    db: &mut Connection,
    query: EventQuery,
    pricer: &super::pricing::Pricer,
    cancel: &dyn Fn() -> bool,
) -> Result<EventPage, String> {
    if query
        .agent
        .as_deref()
        .is_some_and(|a| !super::AGENTS.contains(&a))
    {
        return Err("不支持的请求记录来源".into());
    }
    if query
        .status
        .as_deref()
        .is_some_and(|s| !["success", "failed"].contains(&s))
    {
        return Err("不支持的请求结果筛选".into());
    }
    if query.from.zip(query.to).is_some_and(|(a, b)| a >= b) {
        return Err("请求记录时间范围无效".into());
    }
    if cancel() {
        return Err("请求记录读取已取消".into());
    }
    let pricing_snapshot = pricer.snapshot();
    // 总数、页数据和版本共用一个快照。
    let tx = db.transaction().map_err(|_| "无法读取请求记录快照")?;
    let revision: String = {
        let mut stmt = tx
            .prepare("SELECT agent,indexed FROM usage_request_index ORDER BY agent")
            .map_err(|error| format!("无法读取请求记录版本：{error}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{}:{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?
                ))
            })
            .map_err(|error| format!("无法读取请求记录版本：{error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法读取请求记录版本：{error}"))?
            .join("|")
    };
    let mut predicates = vec![DISTINCT_REQUESTS.to_owned()];
    let mut values = Vec::<rusqlite::types::Value>::new();
    if let Some(agent) = &query.agent {
        predicates.push("agent=?".into());
        values.push(agent.clone().into());
    }
    if let Some(time) = query.from {
        predicates.push("time>=?".into());
        values.push(time.into());
    }
    if let Some(time) = query.to {
        predicates.push("time<?".into());
        values.push(time.into());
    }
    let filters = query
        .include_filters
        .then(|| filter_counts(&tx, &predicates.join(" AND "), &values, &query, cancel))
        .transpose()?;
    for (column, value) in [("model", query.model), (PROJECT, query.project)] {
        if let Some(value) = value {
            predicates.push(format!("{column}=?"));
            values.push(value.into());
        }
    }
    if let Some(status) = query.status {
        predicates.push(if status == "failed" {
            IS_FAILED.into()
        } else {
            format!("NOT ({IS_FAILED})")
        });
    }
    let filter = predicates.join(" AND ");
    let timeline = timeline(
        &tx,
        &filter,
        &values,
        query.from,
        query.to,
        chrono::Utc::now().timestamp_millis(),
    )?;
    let total = timeline.len();
    let (has_cache_writes, has_service_tiers) = tx
        .query_row(
            &format!(
                "SELECT
                EXISTS(SELECT 1 FROM usage_request_events WHERE {filter} AND {CACHE_WRITES}),
                EXISTS(SELECT 1 FROM usage_request_events WHERE {filter} AND {SERVICE_TIER})"
            ),
            params_from_iter(values.iter().chain(&values)),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| "无法读取请求列信息")?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let pages = total.div_ceil(u64::from(limit)).max(1);
    let mut page = query.page.unwrap_or(1).clamp(1, pages);
    if let Some(anchor) = query.anchor {
        let mut anchor_values = values.clone();
        anchor_values.push(anchor.clone().into());
        let time: Option<i64> = tx
            .query_row(
                &format!("SELECT time FROM usage_request_events WHERE {filter} AND id=?"),
                params_from_iter(&anchor_values),
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "无法定位当前请求")?;
        if let Some(time) = time {
            let mut before_values = values.clone();
            before_values.extend([time.into(), time.into(), anchor.into()]);
            let before: i64 = tx.query_row(
                &format!("SELECT count(*) FROM usage_request_events WHERE {filter} AND (time>? OR (time=? AND id>?))"),
                params_from_iter(&before_values), |r| r.get(0),
            ).map_err(|_| "无法定位请求所在页")?;
            page = before as u64 / u64::from(limit) + 1;
        }
        // 归并可能替换旧请求 ID；找不到锚点时保留原页码并按新总量收敛。
    }
    let offset = (page - 1) * u64::from(limit);
    values.extend([i64::from(limit).into(), (offset as i64).into()]);
    let mut events: Vec<RequestEvent> = Vec::new();
    {
        let mut stmt = tx
            .prepare(&format!(
                "SELECT data FROM usage_request_events WHERE {filter} ORDER BY time DESC,id DESC LIMIT ? OFFSET ?"
            ))
            .map_err(|_| "无法读取请求记录")?;
        let rows = stmt
            .query_map(params_from_iter(&values), |r| r.get::<_, String>(0))
            .map_err(|_| "无法读取请求记录")?;
        for row in rows {
            if cancel() {
                return Err("请求记录读取已取消".into());
            }
            let mut event: RequestEvent =
                serde_json::from_str(&row.map_err(|_| "无法读取请求记录")?)
                    .map_err(|_| "请求索引损坏，请重试同步")?;
            if event.agent == "codex" {
                event.request.normalize_codex_kind();
                if event.request.timing.as_deref() == Some(LEGACY_ESTIMATE) {
                    event.request.clear_legacy_codex_timing();
                    event.output_tps = None;
                }
            }
            if event.request.scope.as_deref() == Some("usage_delta") {
                event.request.clear_timing();
                event.output_tps = None;
            }
            match pricing_snapshot.price_event(&event) {
                Ok(cost) => event.cost = Some(cost),
                Err(issue) => event.pricing_issue = Some(issue),
            }
            events.push(event);
        }
    }
    Ok(EventPage {
        events,
        total,
        has_cache_writes,
        has_service_tiers,
        page,
        page_size: limit,
        revision,
        pricing_revision: pricing_snapshot.revision,
        timeline,
        filters,
    })
}
