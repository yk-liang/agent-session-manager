use super::{
    catalog::{self, model_key, MatchOutcome, SourceCatalog},
    litellm, models_dev,
    types::*,
};
use crate::usage::events::RequestEvent;
use chrono::NaiveDate;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub(in crate::usage) const BUILTIN_SOURCE: &str = "builtin";
pub(in crate::usage) const NO_USAGE_SOURCE: &str = "no_usage";

fn is_free_model(model: &str) -> bool {
    let model = model.trim();
    model
        .get(model.len().saturating_sub(5)..)
        .is_some_and(|suffix| {
            suffix.eq_ignore_ascii_case("-free") || suffix.eq_ignore_ascii_case(":free")
        })
}

#[derive(Clone)]
pub struct PricingSnapshot {
    pub revision: u64,
    pub(super) custom: Arc<BTreeMap<String, CustomPriceRecord>>,
    pub(super) models_dev: Option<Arc<SourceCatalog>>,
    pub(super) litellm: Option<Arc<SourceCatalog>>,
}

enum ReferenceMatch {
    Absent,
    Conflicting,
    Priced(ReferenceSummary, BTreeSet<String>),
}

impl ReferenceMatch {
    fn priced(self) -> Option<(ReferenceSummary, BTreeSet<String>)> {
        match self {
            Self::Priced(price, tiers) => Some((price, tiers)),
            _ => None,
        }
    }
}

impl PricingSnapshot {
    pub fn resolve(
        &self,
        model: &str,
        provider_hint: Option<&str>,
        at_ms: i64,
    ) -> Option<ResolvedPrice> {
        let key = model_key(model);
        if key.is_empty() || key == "unknown" {
            return None;
        }
        let (prefix, _) = catalog::split_provider_prefix(&key);
        let provider = prefix.or(provider_hint);
        let bare = key.rsplit('/').next().unwrap_or(&key);
        let base = base_model(bare);
        let base = snapshot_base(base).unwrap_or(base);
        let alias = alias_target(base, at_ms);
        let custom = |model: &str, hint| {
            self.lookup_custom(model, hint)
                .map(|record| (record, (model != key).then(|| model.to_owned())))
        };
        let exact_custom = custom(&key, provider_hint)
            .or_else(|| (key != bare).then(|| custom(bare, provider)).flatten());
        if is_free_model(&key) {
            return Some(if let Some((record, matched_as)) = exact_custom {
                custom_to_resolved(record, matched_as, None)
            } else {
                ResolvedPrice {
                    source: BUILTIN_SOURCE.into(),
                    currency: Currency::Usd,
                    rates: Rates {
                        cache_read: Some(0.0),
                        cache_write: Some(0.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });
        }
        let reference = |model: &str| {
            let qualified = key
                .strip_suffix(bare)
                .filter(|prefix| !prefix.is_empty())
                .map(|prefix| format!("{prefix}{model}"));
            self.lookup_reference(&key, qualified.as_deref().unwrap_or(model), provider)
        };
        let exact = reference(bare);
        if let Some((record, matched_as)) = exact_custom {
            let underlying = match exact {
                ReferenceMatch::Absent if base != bare => reference(base),
                exact => exact,
            }
            .priced()
            .or_else(|| alias.and_then(|id| reference(id).priced()));
            return Some(custom_to_resolved(
                record,
                matched_as,
                underlying.map(|(price, _)| price),
            ));
        }
        match exact {
            ReferenceMatch::Priced(price, tiers) => {
                return Some(reference_to_resolved(price, tiers))
            }
            ReferenceMatch::Absent if base != bare => {
                let reference = reference(base);
                if let Some((record, matched_as)) = custom(base, provider) {
                    return Some(custom_to_resolved(
                        record,
                        matched_as,
                        reference.priced().map(|(price, _)| price),
                    ));
                }
                if let Some((price, tiers)) = reference.priced() {
                    return Some(reference_to_resolved(price, tiers));
                }
            }
            _ => {}
        }
        let alias = alias?;
        let reference = reference(alias).priced();
        if let Some((record, matched_as)) = custom(alias, provider) {
            return Some(custom_to_resolved(
                record,
                matched_as,
                reference.map(|(price, _)| price),
            ));
        }
        reference.map(|(price, tiers)| reference_to_resolved(price, tiers))
    }

    #[cfg(test)]
    pub fn evaluate_event_cost(&self, event: &RequestEvent) -> Option<EventCost> {
        self.price_event(event).ok()
    }

    pub fn price_event(&self, event: &RequestEvent) -> Result<EventCost, PricingIssue> {
        self.price(BillableEvent {
            agent: &event.agent,
            time: event.time,
            model: &event.model,
            tokens: &event.tokens,
            details: &event.details,
            provider: event.request.provider.as_deref(),
            tier: event.request.tier.as_deref(),
            failed: event.status == crate::usage::request_parse::RequestOutcome::Failed,
        })
    }

    pub fn price(&self, event: BillableEvent<'_>) -> Result<EventCost, PricingIssue> {
        if event.failed && event.tokens.total() == 0
            && event.tokens.cache_read_tokens == 0
            && event.tokens.cache_write_tokens == 0
            && event.tokens.reasoning_tokens == 0
        {
            return Ok(EventCost {
                source: NO_USAGE_SOURCE.into(),
                ..Default::default()
            });
        }
        let price = self.resolve(event.model, event.provider, event.time);
        if price
            .as_ref()
            .is_some_and(|price| price.source == BUILTIN_SOURCE)
        {
            return Ok(EventCost {
                source: BUILTIN_SOURCE.into(),
                ..Default::default()
            });
        }

        let tier = event.tier.map(|name| name.trim().to_ascii_lowercase());
        let tier = match (event.agent, tier.as_deref()) {
            ("codex", Some("fast")) => Some("priority"),
            (_, tier) => service_tier(tier),
        };
        if let Some(price) = &price {
            let rates = &price.rates;
            if tier.is_none() && rates.long_context.is_none()
                && rates.input == 0.0 && rates.output == 0.0
                && [rates.cache_read, rates.cache_write, rates.cache_write_1h]
                    .into_iter().flatten().all(|rate| rate == 0.0)
            {
                return Ok(EventCost {
                    source: price.source.clone(),
                    currency: price.currency,
                    ..Default::default()
                });
            }
        }

        let details = event.details;
        if !details.input_recorded
            || !details.output_recorded
            || details.input_incomplete
            || details.output_incomplete
        {
            return Err(PricingIssue::UsageIncomplete);
        }
        if event.tokens.unclassified_tokens > 0 {
            return Err(PricingIssue::Unclassified);
        }
        let tokens = event.tokens;
        let cache_total = tokens
            .cache_read_tokens
            .checked_add(tokens.cache_write_tokens)
            .ok_or(PricingIssue::UsageIncomplete)?;
        if event.agent == "qoder" && cache_total > 0 {
            return Err(PricingIssue::CacheAmbiguous);
        }
        // 账本中的总输入包含缓存读写。
        let uncached_input = tokens
            .input_tokens
            .checked_sub(cache_total)
            .ok_or(PricingIssue::UsageIncomplete)?;

        let price = price.ok_or(PricingIssue::Unpriced)?;

        let base_long = price.rates.long_context.as_ref();
        let above_base = base_long.is_some_and(|long| tokens.input_tokens > long.threshold);
        let (rates, multiplier) = match tier {
            None => (price.rates.standard(), 1.0),
            Some(name) => match price.rates.service_tiers.get(name) {
                Some(rates) if !above_base || rates.long_context.is_some() => (rates.clone(), 1.0),
                _ if name == "priority" && event.agent == "codex" => (
                    price.rates.standard(),
                    codex_priority_multiplier(&price).ok_or(PricingIssue::UnsupportedTier)?,
                ),
                _ => return Err(PricingIssue::UnsupportedTier),
            },
        };
        let (input, output, read, write, long_context) = match &rates.long_context {
            Some(long) if tokens.input_tokens > long.threshold => (
                long.input,
                long.output,
                long.cache_read,
                long.cache_write,
                true,
            ),
            _ => (
                rates.input,
                rates.output,
                rates.cache_read,
                rates.cache_write,
                false,
            ),
        };
        let input_rate = input * multiplier;
        let output_rate = output * multiplier;
        let cache_read_rate = read.unwrap_or(input) * multiplier;
        let cache_write_rate = write.unwrap_or(input) * multiplier;

        // Anthropic 的 1 小时缓存写入可按输入价的两倍估算。
        let write_1h = tokens.cache_write_1h_tokens.min(tokens.cache_write_tokens);
        let cache_write_1h_rate = if write_1h == 0 {
            None
        } else if let Some(rate) = price
            .rates
            .cache_write_1h
            .filter(|_| tier.is_none() && !long_context)
        {
            Some(rate)
        } else if event.agent == "claude" || is_anthropic(event.model, event.provider, &price) {
            Some(input_rate * 2.0)
        } else {
            return Err(PricingIssue::UnsupportedComponent);
        };

        let input_usd = per_million(uncached_input, input_rate);
        let cache_read_usd = per_million(tokens.cache_read_tokens, cache_read_rate);
        let cache_write_usd = per_million(tokens.cache_write_tokens - write_1h, cache_write_rate);
        let cache_write_1h_usd = cache_write_1h_rate.map(|rate| per_million(write_1h, rate));
        // 推理 Token 已包含在输出中。
        let output_usd = per_million(tokens.output_tokens, output_rate);
        let total_usd = input_usd
            + cache_read_usd
            + cache_write_usd
            + cache_write_1h_usd.unwrap_or(0.0)
            + output_usd;
        if !total_usd.is_finite() || total_usd < 0.0 {
            return Err(PricingIssue::Unpriced);
        }

        Ok(EventCost {
            total_usd,
            source: if multiplier == 1.0 && tier.is_some_and(|name| price.litellm_tiers.contains(name)) {
                litellm::SOURCE_ID.into()
            } else {
                price.source
            },
            currency: price.currency,
            input_usd,
            cache_read_usd,
            cache_write_usd,
            output_usd,
            input_rate,
            output_rate,
            cache_read_rate,
            cache_write_rate,
            cache_write_1h_usd,
            cache_write_1h_rate,
            long_context,
            service_tier: tier.map(str::to_owned),
            multiplier: (multiplier != 1.0).then_some(multiplier),
        })
    }

    fn lookup_custom(
        &self,
        model: &str,
        provider_hint: Option<&str>,
    ) -> Option<&CustomPriceRecord> {
        let key = model_key(model);
        if !key.contains('/') {
            if let Some(provider) = provider_hint.map(str::trim).filter(|p| !p.is_empty()) {
                let scoped = format!("{}/{}", provider.to_ascii_lowercase(), key);
                if let Some(record) = self.custom.get(&scoped) {
                    return Some(record);
                }
            }
        }
        self.custom.get(&key)
    }

    fn lookup_reference(&self, key: &str, model: &str, provider: Option<&str>) -> ReferenceMatch {
        let provider = provider
            .filter(|provider| !provider.trim().is_empty())
            .map(catalog::normalize_provider);
        let sources = [
            (models_dev::SOURCE_ID, self.models_dev.as_deref()),
            (litellm::SOURCE_ID, self.litellm.as_deref()),
        ];
        let first_party_fallback = provider.as_deref().is_some_and(|provider| {
            !matches!(
                provider,
                "ollama"
                    | "lmstudio"
                    | "local"
                    | "openrouter"
                    | "azure"
                    | "github-copilot"
                    | "bedrock"
                    | "vertex_ai"
            ) && !sources
                .iter()
                .any(|(_, source)| source.is_some_and(|catalog| catalog.has_provider(provider)))
        });
        let bare = model.rsplit('/').next().unwrap_or(model);
        for (source_id, catalog) in sources {
            let Some(catalog) = catalog else {
                continue;
            };
            let matched = match catalog.match_model(model, provider.as_deref()) {
                MatchOutcome::Absent if first_party_fallback => catalog.match_first_party(bare),
                matched => matched,
            };
            match matched {
                MatchOutcome::Absent | MatchOutcome::ListedUnpriced => {}
                MatchOutcome::Conflicting => return ReferenceMatch::Conflicting,
                MatchOutcome::Priced {
                    provider,
                    model_id,
                    mut rates,
                } => {
                    let mut tiers = BTreeSet::new();
                    if source_id == models_dev::SOURCE_ID {
                        if let Some(MatchOutcome::Priced { rates: other, .. }) =
                            self.litellm.as_ref().map(|catalog| {
                                catalog.match_model(&format!("{provider}/{model_id}"), None)
                            })
                        {
                            rates.cache_write_1h = rates.cache_write_1h.or(other.cache_write_1h);
                            rates.cache_write_12h = rates.cache_write_12h.or(other.cache_write_12h);
                            for (tier, rate) in other.service_tiers {
                                if !rates.service_tiers.contains_key(&tier) {
                                    tiers.insert(tier.clone());
                                    rates.service_tiers.insert(tier, rate);
                                }
                            }
                        }
                    }
                    let matched_as = (model != key || first_party_fallback && model != bare)
                        .then(|| bare.to_owned());
                    return ReferenceMatch::Priced(
                        ReferenceSummary {
                            source: source_id.into(),
                            provider,
                            model_id,
                            rates,
                            matched_as,
                        },
                        tiers,
                    );
                }
            }
        }
        ReferenceMatch::Absent
    }
}

/// 日志中的服务档位：None 为标准档。Claude 的 `speed: fast` 等未知档位原样返回，
/// 由调用方按“目录未定价的档位”处理，不会套用标准价。
fn service_tier(tier: Option<&str>) -> Option<&str> {
    let tier = tier.map(str::trim).filter(|t| !t.is_empty())?;
    match tier {
        "standard" | "default" | "auto" | "on_demand" => None,
        "batches" => Some("batch"),
        _ => Some(tier),
    }
}

fn codex_priority_multiplier(price: &ResolvedPrice) -> Option<f64> {
    if price.provider.as_deref().map(catalog::normalize_provider).as_deref() != Some("openai") {
        return None;
    }
    let model = catalog::bare_model_key(price.model_id.as_deref()?);
    match model.as_str() {
        "gpt-5.5" => Some(2.5),
        "gpt-5.4" | "gpt-5.3-codex" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna"
        | "gpt-6-astra" | "gpt-6-sol" | "gpt-6.1-sol" | "gpt-6.1-terra" | "gpt-6-luna" => Some(2.0),
        _ => None,
    }
}

fn is_anthropic(model: &str, provider: Option<&str>, price: &ResolvedPrice) -> bool {
    let creator =
        |id: &str| catalog::inferred_creator(&catalog::bare_model_key(id)) == Some("anthropic");
    creator(model)
        || price.model_id.as_deref().is_some_and(creator)
        || [provider, price.provider.as_deref()]
            .into_iter()
            .flatten()
            .any(|p| catalog::normalize_provider(p) == "anthropic")
}

fn per_million(tokens: u64, rate_per_million: f64) -> f64 {
    tokens as f64 * rate_per_million / 1_000_000.0
}

fn day_ms(date: &str) -> i64 {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc().timestamp_millis())
        .unwrap_or(0)
}

fn alias_target(key: &str, at_ms: i64) -> Option<&'static str> {
    match key {
        "codex-auto-review" => {
            let timeline = [
                (day_ms("2026-07-30"), "gpt-5.6-luna"),
                (day_ms("2026-03-05"), "gpt-5.4"),
                (day_ms("2026-02-05"), "gpt-5.3-codex"),
                (day_ms("2025-12-11"), "gpt-5.2-codex"),
                (day_ms("2025-11-13"), "gpt-5.1-codex"),
                (day_ms("2025-09-15"), "gpt-5-codex"),
                (0, "gpt-5"),
            ];
            timeline
                .iter()
                .find(|(since, _)| at_ms >= *since)
                .map(|(_, target)| *target)
        }
        "deepseek-v4.1-flash" => Some("deepseek-flash"),
        "gemini-4-argon-trial" => Some("gemini-4-argon"),
        _ => None,
    }
}

fn base_model(model: &str) -> &str {
    model
        .rsplit_once('-')
        .and_then(|(base, suffix)| {
            (matches!(suffix, "low" | "medium" | "high" | "xhigh")
                || (base.starts_with("gemini-")
                    && suffix.len() == 1
                    && suffix.as_bytes()[0].is_ascii_lowercase()))
            .then_some(base)
        })
        .unwrap_or(model)
}

fn snapshot_base(model: &str) -> Option<&str> {
    if model == "deepseek-v4.1-flash-1017" {
        return Some("deepseek-v4.1-flash");
    }
    for (length, format) in [(8, "%Y%m%d"), (10, "%Y-%m-%d")] {
        let Some((base, suffix)) = model.split_at_checked(model.len().saturating_sub(length)) else {
            continue;
        };
        if let Some(base) = base.strip_suffix('-') {
            if suffix.starts_with("20") && NaiveDate::parse_from_str(suffix, format).is_ok() {
                return Some(base);
            }
        }
    }
    None
}

fn reference_to_resolved(
    price: ReferenceSummary,
    litellm_tiers: BTreeSet<String>,
) -> ResolvedPrice {
    ResolvedPrice {
        source: price.source,
        provider: Some(price.provider),
        model_id: Some(price.model_id),
        matched_as: price.matched_as,
        currency: Currency::Usd,
        original_rates: None,
        rates: price.rates,
        overrides_reference: None,
        litellm_tiers,
    }
}

fn custom_to_resolved(
    record: &CustomPriceRecord,
    matched_as: Option<String>,
    overrides_reference: Option<ReferenceSummary>,
) -> ResolvedPrice {
    ResolvedPrice {
        source: "custom".into(),
        provider: None,
        model_id: Some(record.model.clone()),
        matched_as,
        currency: record.currency,
        original_rates: Some(record.original_rates()),
        rates: record.to_usd_rates(),
        overrides_reference,
        litellm_tiers: BTreeSet::new(),
    }
}
