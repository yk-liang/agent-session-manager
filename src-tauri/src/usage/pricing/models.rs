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
        #[derive(Default)]
        struct ProviderUsage {
            agents: BTreeSet<String>,
            requests: u64,
            total_tokens: u64,
        }
        let mut by_model_provider: BTreeMap<String, BTreeMap<Option<String>, ProviderUsage>> =
            BTreeMap::new();
        {
            let mut stmt = db
                .prepare(
                    &format!("SELECT agent, model, nullif(trim(json_extract(data, '$.request.provider')),'') AS provider,
                        COUNT(*), SUM(COALESCE(json_extract(data, '$.inputTokens'), 0)
                                   + COALESCE(json_extract(data, '$.outputTokens'), 0)
                                   + COALESCE(json_extract(data, '$.unclassifiedTokens'), 0))
                     FROM usage_request_events
                     WHERE (?1 IS NULL OR agent = ?1) AND model <> 'unknown' AND trim(model) <> ''
                       AND {DISTINCT_REQUESTS}
                     GROUP BY agent, model, provider"),
                )
                .map_err(|_| "无法查询模型价格列表")?;
            let mut rows = stmt
                .query([agent_filter])
                .map_err(|_| "无法查询模型价格列表")?;
            while let Some(row) = rows.next().map_err(|_| "无法读取模型记录")? {
                let agent: String = row.get(0).map_err(|_| "模型来源无效")?;
                let model: String = row.get(1).map_err(|_| "模型 ID 无效")?;
                let provider = row
                    .get::<_, Option<String>>(2)
                    .map_err(|_| "模型提供商无效")?;
                let requests: i64 = row.get(3).map_err(|_| "请求计数无效")?;
                let tokens: i64 = row.get(4).map_err(|_| "模型 Token 计数无效")?;
                let tokens = u64::try_from(tokens).map_err(|_| "模型 Token 计数无效")?;
                let usage = by_model_provider
                    .entry(model)
                    .or_default()
                    .entry(provider)
                    .or_default();
                usage.agents.insert(agent);
                usage.requests = usage.requests.saturating_add(requests as u64);
                usage.total_tokens = usage.total_tokens.saturating_add(tokens);
            }
        }

        let now = Utc::now().timestamp_millis();
        let (snapshot, catalog) = {
            let state = self.state.read().unwrap_or_else(|e| e.into_inner());
            (Arc::clone(&state.snapshot), state.status(now))
        };
        let mut models = Vec::with_capacity(by_model_provider.len() + snapshot.custom.len());

        for (model, provider_buckets) in by_model_provider {
            let resolved_buckets: Vec<(Option<String>, ProviderUsage, Option<ResolvedPrice>)> =
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
                let provider = match resolved_buckets.as_slice() {
                    [(provider, _, _)] => provider.clone(),
                    _ => None,
                };
                let mut agents = BTreeSet::new();
                let mut providers = BTreeSet::new();
                let mut requests = 0u64;
                let mut total_tokens = 0u64;
                let mut chosen_price = None;
                for (provider, usage, price) in resolved_buckets {
                    agents.extend(usage.agents);
                    if let Some(p) = provider {
                        providers.insert(p);
                    }
                    requests += usage.requests;
                    total_tokens = total_tokens.saturating_add(usage.total_tokens);
                    if chosen_price.is_none() {
                        chosen_price = price;
                    }
                }
                models.push(ModelPriceRow {
                    model,
                    provider,
                    agents: agents.into_iter().collect(),
                    providers: providers.into_iter().collect(),
                    requests,
                    total_tokens,
                    price: chosen_price,
                    custom_only: false,
                });
            } else {
                // 同名模型的价格不同时，按提供商分列。
                for (provider, usage, price) in resolved_buckets {
                    models.push(ModelPriceRow {
                        model: model.clone(),
                        provider: provider.clone(),
                        agents: usage.agents.into_iter().collect(),
                        providers: provider.into_iter().collect(),
                        requests: usage.requests,
                        total_tokens: usage.total_tokens,
                        price,
                        custom_only: false,
                    });
                }
            }
        }

        let editable: BTreeSet<_> = models
            .iter()
            .filter_map(|row| {
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
                .map(|(_, record)| ModelPriceRow {
                    model: record.model.clone(),
                    provider: None,
                    agents: Vec::new(),
                    providers: Vec::new(),
                    requests: 0,
                    total_tokens: 0,
                    price: snapshot.resolve(&record.model, None, now),
                    custom_only: true,
                }),
        );

        models.sort_by(|a, b| {
            b.requests
                .cmp(&a.requests)
                .then_with(|| b.total_tokens.cmp(&a.total_tokens))
                .then_with(|| a.model.cmp(&b.model))
                .then_with(|| a.provider.cmp(&b.provider))
        });

        Ok(ModelPricesResponse { models, catalog })
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
