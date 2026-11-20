//! 按时间范围、Agent 和模型等维度汇总已入账请求。
use super::{events, types::*, Store};
use chrono::{Local, TimeZone};
use rusqlite::{params_from_iter, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SummaryQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    /// 前端按本地日历换算的时间戳边界，保留夏令时和非整点时区。
    pub bounds: Vec<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRow {
    pub agent: String,
    pub bucket_start: Option<i64>,
    pub record_count: u64,
    #[serde(flatten)]
    pub usage: UsageRow,
    #[serde(flatten)]
    pub metrics: RequestMetrics,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestMetrics {
    /// 仅计可辨认的单次请求，含失败请求。累计差额不计入次数。
    pub request_count: u64,
    pub request_count_incomplete: bool,
    pub success_count: u64,
    pub failed_count: u64,
    /// 按模型价格估算的美元费用。无可用估算时为空。
    pub cost_usd: Option<f64>,
    pub custom_cost_usd: Option<f64>,
    pub reference_cost_usd: Option<f64>,
    pub cost_unknown_count: u64,
    /// 模型无可用定价的请求数。
    pub unpriced_count: u64,
    /// 计量字段缺失、只有下限或无法拆分，无法估算费用的请求数。
    pub cost_usage_incomplete_count: u64,
    /// 服务档位或计费分项（如 flex、1 小时缓存写入）没有对应价格的请求数。
    pub cost_special_count: u64,
    pub cost_incomplete: bool,
    pub cache_read_incomplete: bool,
    /// 可拆分输入的计量子集。
    pub cache_input_tokens: Option<u64>,
    pub cache_input_read_tokens: u64,
    pub cache_input_write_tokens: u64,
    pub cache_excluded_count: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimensionSummaryRow {
    pub agent: String,
    pub model: String,
    pub key: String,
    pub record_count: u64,
    #[serde(flatten)]
    pub tokens: Tokens,
    #[serde(flatten)]
    pub details: RecordedFields,
    pub total_tokens: u64,
    pub partial: bool,
    #[serde(flatten)]
    pub metrics: RequestMetrics,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub rows: Vec<SummaryRow>,
    pub projects: Vec<DimensionSummaryRow>,
    pub providers: Vec<DimensionSummaryRow>,
    /// 此快照中已完成过完整目录同步的来源，包含已确认的空目录。
    /// 有请求记录不代表首次导入已结束；失败或仅同步指定文件不能建立此基线。
    pub synced_agents: Vec<String>,
    /// 各来源缺少发生时间的记录数（与查询范围无关）；有时间范围的查询不包含它们。
    pub untimed: BTreeMap<String, u64>,
    pub revision: String,
    pub pricing_revision: u64,
}

pub fn read(query: SummaryQuery, cancel: &dyn Fn() -> bool) -> Result<Summary, String> {
    if cancel() {
        return Err("用量查询已取消".into());
    }
    let mut db = super::ledger::reader(&super::ledger::path()?, cancel)?;
    read_at(&mut db, query, cancel)
}

#[allow(dead_code)] // 供隔离目录下的 usage_perf 示例复测，GUI 使用独立只读连接。
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
    read_at_with_pricer(db, query, super::pricing::global(), cancel)
}

pub(super) fn read_at_with_pricer(
    db: &mut Connection,
    query: SummaryQuery,
    pricer: &super::pricing::Pricer,
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
    if cancel() {
        return Err("用量查询已取消".into());
    }
    let tx = db
        .transaction()
        .map_err(|error| format!("无法读取用量快照：{error}"))?;
    let synced_agents = {
        let mut stmt = tx
            .prepare("SELECT agent FROM usage_sync_state ORDER BY agent")
            .map_err(|_| "无法读取用量同步状态")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|_| "无法读取用量同步状态")?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "无法读取用量同步状态")?
    };
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
    let pricing_snapshot = pricer.snapshot();
    let pricing_revision = pricing_snapshot.revision;
    let mut sql = format!(
        "SELECT agent,time,model,{},data FROM usage_request_events WHERE {}",
        events::IS_FAILED,
        events::DISTINCT_REQUESTS
    );
    let mut values = Vec::<rusqlite::types::Value>::new();
    if let Some((from, to)) = query.from.zip(query.to) {
        sql.push_str(" AND time>=? AND time<?");
        values.extend([from.into(), to.into()]);
    }
    let mut stmt = tx.prepare(&sql).map_err(|_| "无法查询请求用量")?;
    let mut rows = stmt
        .query(params_from_iter(values))
        .map_err(|_| "无法查询请求用量")?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Measurement {
        #[serde(flatten)]
        tokens: Tokens,
        #[serde(flatten)]
        details: RecordedFields,
        partial: bool,
        #[serde(default)]
        request: MeasurementRequest,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct MeasurementRequest {
        provider: Option<String>,
        project: Option<String>,
        tier: Option<String>,
        scope: Option<String>,
    }
    #[derive(Default)]
    struct Group {
        record_count: u64,
        tokens: Tokens,
        details: RecordedFields,
        partial: bool,
        metrics: RequestMetrics,
    }
    impl Group {
        fn add(
            &mut self,
            m: &Measurement,
            failed: bool,
            measured: bool,
            cache_input_available: bool,
            evaluated: Result<&super::pricing::EventCost, super::pricing::PricingIssue>,
        ) {
            self.record_count += 1;
            // 无 Token 计量的失败仍计入请求次数。
            if measured {
                self.tokens.add(&m.tokens);
                self.details.merge(&m.details);
                self.metrics.cache_read_incomplete |= !m.details.cache_read_recorded;
            }
            if cache_input_available {
                let input = self.metrics.cache_input_tokens.get_or_insert(0);
                *input = input.saturating_add(m.tokens.input_tokens);
                self.metrics.cache_input_read_tokens = self.metrics.cache_input_read_tokens
                    .saturating_add(m.tokens.cache_read_tokens);
                self.metrics.cache_input_write_tokens = self.metrics.cache_input_write_tokens
                    .saturating_add(m.tokens.cache_write_tokens);
            } else if measured || !failed {
                self.metrics.cache_excluded_count += 1;
            }
            self.partial |= m.partial;
            if matches!(m.request.scope.as_deref(), Some("request" | "error")) {
                self.metrics.request_count += 1;
                if failed {
                    self.metrics.failed_count += 1;
                } else {
                    self.metrics.success_count += 1;
                }
            } else {
                self.metrics.request_count_incomplete = true;
            }
            if let Ok(cost) = evaluated {
                *self.metrics.cost_usd.get_or_insert(0.0) += cost.total_usd;
                match cost.source.as_str() {
                    super::pricing::BUILTIN_SOURCE | super::pricing::NO_USAGE_SOURCE => {}
                    "custom" => {
                        *self.metrics.custom_cost_usd.get_or_insert(0.0) += cost.total_usd;
                    }
                    _ => {
                        *self.metrics.reference_cost_usd.get_or_insert(0.0) += cost.total_usd;
                    }
                }
            } else if let Err(issue) = evaluated {
                use super::pricing::PricingIssue::*;
                self.metrics.cost_incomplete = true;
                self.metrics.cost_unknown_count += 1;
                *match issue {
                    Unpriced => &mut self.metrics.unpriced_count,
                    UsageIncomplete | Unclassified => &mut self.metrics.cost_usage_incomplete_count,
                    CacheAmbiguous | UnsupportedTier | UnsupportedComponent => {
                        &mut self.metrics.cost_special_count
                    }
                } += 1;
            }
        }
    }
    let mut grouped = BTreeMap::<(String, String, String, Option<i64>), Group>::new();
    let mut projects_grouped = BTreeMap::<(String, String, String), Group>::new();
    let mut providers_grouped = BTreeMap::<(String, String, String), Group>::new();
    while let Some(row) = rows.next().map_err(|_| "无法读取请求计量")? {
        if cancel() {
            return Err("用量查询已取消".into());
        }
        let agent: String = row.get(0).map_err(|_| "请求来源无效")?;
        let time: i64 = row.get(1).map_err(|_| "请求时间无效")?;
        let model: String = row.get(2).map_err(|_| "请求模型无效")?;
        let failed: bool = row.get(3).map_err(|_| "请求状态无效")?;
        let data: String = row.get(4).map_err(|_| "请求计量无效")?;
        let m: Measurement =
            serde_json::from_str(&data).map_err(|_| "请求计量损坏，已保留原数据")?;

        let measured =
            m.details.input_recorded || m.details.output_recorded || m.tokens.total() > 0;
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
        let cache_input_available = agent != "qoder"
            && m.details.input_recorded
            && !m.details.input_incomplete
            && m.details.cache_read_recorded
            && m.tokens.cache_read_tokens.checked_add(m.tokens.cache_write_tokens)
                .is_some_and(|cached| cached <= m.tokens.input_tokens);

        let billable = super::pricing::BillableEvent {
            agent: &agent,
            time,
            model: &model,
            tokens: &m.tokens,
            details: &m.details,
            provider: m.request.provider.as_deref(),
            tier: m.request.tier.as_deref(),
            failed,
        };
        let evaluated = pricing_snapshot.price(billable);
        let project_key = m.request.project.clone().unwrap_or_default();
        let provider_key = m.request.provider.clone().unwrap_or_default();
        for group in [
            grouped
                .entry((agent.clone(), model.clone(), date, bucket_start))
                .or_default(),
            projects_grouped
                .entry((agent.clone(), model.clone(), project_key))
                .or_default(),
        ] {
            group.add(
                &m,
                failed,
                measured,
                cache_input_available,
                evaluated.as_ref().map_err(|e| *e),
            );
        }
        providers_grouped
            .entry((agent, model, provider_key))
            .or_default()
            .add(
                &m,
                failed,
                measured,
                cache_input_available,
                evaluated.as_ref().map_err(|e| *e),
            );
    }
    drop(rows);
    drop(stmt);
    let untimed = {
        let mut stmt = tx
            .prepare("SELECT agent,count(*) FROM usage_request_events WHERE time<=0 GROUP BY agent")
            .map_err(|_| "无法统计缺少时间的请求")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|_| "无法统计缺少时间的请求")?;
        rows.map(|row| row.map(|(agent, n)| (agent, n.max(0) as u64)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(|_| "无法统计缺少时间的请求")?
    };
    let rows = grouped
        .into_iter()
        .map(|((agent, model, date, bucket_start), group)| SummaryRow {
            agent,
            bucket_start,
            record_count: group.record_count,
            usage: UsageRow {
                date,
                model,
                total_tokens: group.tokens.total(),
                tokens: group.tokens,
                details: group.details,
                partial: group.partial,
            },
            metrics: group.metrics,
        })
        .collect();
    let to_dimension =
        |map: BTreeMap<(String, String, String), Group>| -> Vec<DimensionSummaryRow> {
            map.into_iter()
                .map(|((agent, model, key), group)| DimensionSummaryRow {
                    agent,
                    model,
                    key,
                    record_count: group.record_count,
                    total_tokens: group.tokens.total(),
                    tokens: group.tokens,
                    details: group.details,
                    partial: group.partial,
                    metrics: group.metrics,
                })
                .collect()
        };
    Ok(Summary {
        synced_agents,
        untimed,
        rows,
        projects: to_dimension(projects_grouped),
        providers: to_dimension(providers_grouped),
        revision,
        pricing_revision,
    })
}
