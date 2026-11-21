use super::{
    catalog::model_key,
    types::{ModelPriceRow, ModelPricesResponse, ResolvedPrice},
    Pricer,
};
use crate::usage::events::DISTINCT_REQUESTS;
use chrono::Utc;
use rusqlite::Connection;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct UsageRank {
    requests: u64,
    total_tokens: u64,
}

impl UsageRank {
    fn add(&mut self, other: Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

impl Pricer {
    pub fn list_model_prices_from_db(
        &self,
        db: &Connection,
        agent_filter: Option<&str>,
    ) -> Result<ModelPricesResponse, String> {
        let agent_filter = agent_filter.filter(|a| !a.is_empty() && *a != "all");
        if let Some(a) = agent_filter {
            if !crate::usage::AGENTS.contains(&a) {
                return Err("不支持的来源筛选".into());
            }
        }
        let mut by_model_provider: BTreeMap<String, BTreeMap<Option<String>, UsageRank>> =
            BTreeMap::new();
        {
            let mut stmt = db
                .prepare(
                    &format!("SELECT coalesce(nullif(trim(model), ''), 'unknown') AS model_name, nullif(trim(json_extract(data, '$.request.provider')),'') AS provider,
                        COUNT(CASE WHEN json_extract(data, '$.request.scope') IN ('request', 'error') THEN 1 END), SUM(COALESCE(json_extract(data, '$.inputTokens'), 0)
                                   + COALESCE(json_extract(data, '$.outputTokens'), 0)
                                   + COALESCE(json_extract(data, '$.unclassifiedTokens'), 0))
                     FROM usage_request_events
                     WHERE (?1 IS NULL OR agent = ?1)
                       AND {DISTINCT_REQUESTS}
                     GROUP BY agent, model_name, provider"),
                )
                .map_err(|_| "无法查询模型价格列表")?;
            let mut rows = stmt
                .query([agent_filter])
                .map_err(|_| "无法查询模型价格列表")?;
            while let Some(row) = rows.next().map_err(|_| "无法读取模型记录")? {
                let model: String = row.get(0).map_err(|_| "模型 ID 无效")?;
                let provider = row
                    .get::<_, Option<String>>(1)
                    .map_err(|_| "模型提供商无效")?;
                let requests: i64 = row.get(2).map_err(|_| "请求计数无效")?;
                let total_tokens: i64 = row.get(3).map_err(|_| "模型 Token 计数无效")?;
                let usage = UsageRank {
                    requests: requests as u64,
                    total_tokens: u64::try_from(total_tokens).map_err(|_| "模型 Token 计数无效")?,
                };
                by_model_provider
                    .entry(model)
                    .or_default()
                    .entry(provider)
                    .or_default()
                    .add(usage);
            }
        }

        let now = Utc::now().timestamp_millis();
        let (snapshot, catalog) = {
            let state = self.state.read().unwrap_or_else(|e| e.into_inner());
            (Arc::clone(&state.snapshot), state.status(now))
        };
        let mut models = Vec::with_capacity(by_model_provider.len() + snapshot.custom.len());

        for (model, provider_buckets) in by_model_provider {
            let resolved_buckets: Vec<_> =
                provider_buckets
                    .into_iter()
                    .map(|(provider, usage)| {
                        let price = snapshot.resolve(&model, provider.as_deref(), now);
                        (provider, usage, price)
                    })
                    .collect();

            let all_same_price = resolved_buckets
                .windows(2)
                .all(|pair| same_resolved_price(pair[0].2.as_ref(), pair[1].2.as_ref()));

            if all_same_price {
                let mut buckets = resolved_buckets.into_iter();
                let Some((provider, mut rank, price)) = buckets.next() else { continue };
                let provider = if buckets.len() == 0 { provider } else { None };
                for (_, usage, _) in buckets {
                    rank.add(usage);
                }
                models.push((ModelPriceRow { model, provider, price }, rank));
            } else {
                models.extend(resolved_buckets.into_iter().map(|(provider, rank, price)| {
                    (ModelPriceRow { model: model.clone(), provider, price }, rank)
                }));
            }
        }

        let editable: BTreeSet<_> = models
            .iter()
            .filter_map(|(row, _)| {
                let price = row.price.as_ref()?;
                (price.source == "custom" && price.matched_as.is_none())
                    .then(|| model_key(price.model_id.as_deref().unwrap_or(&row.model)))
            })
            .collect();
        models.extend(
            snapshot
                .custom
                .iter()
                .filter(|(key, _)| !editable.contains(*key))
                .map(|(_, record)| (ModelPriceRow {
                    model: record.model.clone(),
                    provider: None,
                    price: snapshot.resolve(&record.model, None, now),
                }, UsageRank::default())),
        );

        models.sort_by(|(a, a_rank), (b, b_rank)| {
            b_rank.cmp(a_rank)
                .then_with(|| a.model.cmp(&b.model))
                .then_with(|| a.provider.cmp(&b.provider))
        });

        Ok(ModelPricesResponse { models: models.into_iter().map(|(row, _)| row).collect(), catalog })
    }
}

fn same_resolved_price(a: Option<&ResolvedPrice>, b: Option<&ResolvedPrice>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.source == y.source
                && x.provider == y.provider
                && x.model_id == y.model_id
                && x.matched_as == y.matched_as
                && x.currency == y.currency
                && x.rates.approx_eq(&y.rates)
        }
        _ => false,
    }
}
