//! LiteLLM Model Cost Map 解析器；公开目录单位为 USD / Token，规范换算为 USD / 1M tokens。
use super::{
    catalog::{
        bare_model_key, inferred_creator, normalize_provider, split_provider_prefix, SourceCatalog,
    },
    types::{
        approx_f64, per_token_to_per_million, LongContextRates, QuoteRates, Rates, ReferenceListing,
        TierRates, TokenRates,
    },
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const SOURCE_ID: &str = "litellm";
pub const SOURCE_NAME: &str = "LiteLLM";

fn opt_f64(map: &Map<String, Value>, key: &str) -> Option<f64> {
    map.get(key).and_then(Value::as_f64)
}

fn supported_mode(map: &Map<String, Value>) -> bool {
    match map.get("mode").and_then(Value::as_str) {
        None | Some("chat") | Some("completion") | Some("responses") => true,
        Some(_) => false,
    }
}

/// 解析形如 `128k_tokens` / `200k_tokens` / `272k_tokens` / `1m_tokens` / `272000_tokens` 的阈值。
fn parse_token_threshold(suffix: &str) -> Option<u64> {
    let body = suffix.strip_suffix("_tokens")?;
    if body.is_empty() {
        return None;
    }
    if let Some(num) = body.strip_suffix('k') {
        num.parse::<u64>().ok()?.checked_mul(1_000)
    } else if let Some(num) = body.strip_suffix('m') {
        num.parse::<u64>().ok()?.checked_mul(1_000_000)
    } else {
        body.parse::<u64>().ok()
    }
    .filter(|&n| n > 0)
}

#[derive(Clone, Copy)]
enum Field {
    Input,
    Output,
    CacheRead,
    CacheWrite,
    CacheWrite1h,
    CacheWrite12h,
}

#[derive(Default)]
struct Fields {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    cache_write_1h: Option<f64>,
    cache_write_12h: Option<f64>,
}

impl Fields {
    fn quote(self) -> Option<QuoteRates> {
        Some(QuoteRates {
            tokens: TokenRates {
                input: self.input?,
                output: self.output?,
                cache_read: self.cache_read,
                cache_write: self.cache_write,
            },
            cache_write_1h: self.cache_write_1h,
            cache_write_12h: self.cache_write_12h,
        })
    }

    fn set(&mut self, field: Field, rate: f64) {
        *match field {
            Field::Input => &mut self.input,
            Field::Output => &mut self.output,
            Field::CacheRead => &mut self.cache_read,
            Field::CacheWrite => &mut self.cache_write,
            Field::CacheWrite1h => &mut self.cache_write_1h,
            Field::CacheWrite12h => &mut self.cache_write_12h,
        } = Some(rate);
    }
}

/// 一个服务档位（标准、priority、flex、batch）的基础费率与按输入阈值的长上下文费率。
#[derive(Default)]
struct TierFields {
    base: Fields,
    above: BTreeMap<u64, Fields>,
}

impl TierFields {
    fn rates(self) -> Option<TierRates> {
        // 只有缓存写入 TTL 的条目不足以定义新的上下文档位。
        let mut above = self.above.into_iter().filter(|(_, rates)| {
            [rates.input, rates.output, rates.cache_read, rates.cache_write].into_iter().any(|rate| rate.is_some())
        });
        let long_context = match above.next() {
            None => None,
            Some((threshold, tier)) => {
                if above.next().is_some() {
                    return None;
                }
                Some(LongContextRates {
                    threshold,
                    rates: tier.quote()?,
                })
            }
        };
        Some(TierRates {
            base: self.base.quote()?,
            long_context,
        })
    }
}

const FIELD_PREFIXES: [(&str, Field); 6] = [
    ("input_cost_per_token", Field::Input),
    ("output_cost_per_token", Field::Output),
    ("cache_read_input_token_cost", Field::CacheRead),
    ("cache_creation_input_token_cost_above_1hr", Field::CacheWrite1h),
    ("cache_creation_input_token_cost_above_12hr", Field::CacheWrite12h),
    ("cache_creation_input_token_cost", Field::CacheWrite),
];
const SERVICE_SUFFIXES: [(&str, &str); 4] = [
    ("_priority", "priority"),
    ("_flex", "flex"),
    ("_batches", "batch"),
    ("_ultrafast", "ultrafast"),
];

fn tier_rates(map: &Map<String, Value>, service_suffix: &str) -> Option<TierRates> {
    let mut tier = TierFields::default();
    for (key, value) in map {
        let Some(key) = key.strip_suffix(service_suffix) else { continue };
        let Some((field, suffix)) = FIELD_PREFIXES
            .iter()
            .find_map(|(prefix, field)| key.strip_prefix(prefix).map(|suffix| (*field, suffix)))
        else {
            continue;
        };
        let fields = if suffix.is_empty() {
            &mut tier.base
        } else if let Some(threshold) = suffix.strip_prefix("_above_").and_then(parse_token_threshold) {
            tier.above.entry(threshold).or_default()
        } else {
            continue;
        };
        if value.is_null() && suffix.is_empty() {
            continue;
        }
        let rate = match value.as_f64().and_then(per_token_to_per_million) {
            Some(rate) => rate,
            None if matches!(field, Field::CacheWrite1h | Field::CacheWrite12h) => continue,
            None => return None,
        };
        fields.set(field, rate);
    }
    let reasoning_key = format!("output_cost_per_reasoning_token{service_suffix}");
    if let Some(reasoning) = map.get(&reasoning_key).filter(|value| !value.is_null()) {
        if !approx_f64(per_token_to_per_million(reasoning.as_f64()?)?, tier.base.output?) {
            return None;
        }
    }
    tier.rates().filter(TierRates::validate)
}

fn parse_rates(map: &Map<String, Value>) -> Option<Rates> {
    if !supported_mode(map)
        || ["input_cost_per_character", "output_cost_per_character"]
            .into_iter()
            .any(|key| opt_f64(map, key).is_some_and(|rate| rate > 0.0))
        || map.get("tiered_pricing").and_then(Value::as_array).is_some_and(|tiers| !tiers.is_empty())
    {
        return None;
    }
    Some(Rates {
        standard: tier_rates(map, "")?,
        service_tiers: SERVICE_SUFFIXES.into_iter()
            .filter_map(|(suffix, name)| Some((name.to_owned(), tier_rates(map, suffix)?)))
            .collect(),
    })
}

pub fn parse(raw: &[u8]) -> Result<SourceCatalog, String> {
    let root: Value =
        serde_json::from_slice(raw).map_err(|error| format!("解析 LiteLLM 目录失败：{error}"))?;
    let object = root
        .as_object()
        .ok_or_else(|| "LiteLLM 目录根节点必须是对象".to_owned())?;

    let mut listings = Vec::new();
    for (entry_key, entry_value) in object {
        if entry_key == "sample_spec" {
            continue;
        }
        let Some(map) = entry_value.as_object() else {
            continue;
        };
        let (prefix, _) = split_provider_prefix(entry_key);
        let provider = map
            .get("litellm_provider")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or(prefix)
            .unwrap_or("unknown")
            .to_owned();
        let normalized_provider = normalize_provider(&provider);
        let bare_id = bare_model_key(entry_key);
        let first_party = inferred_creator(&bare_id) == Some(normalized_provider.as_str())
            && !entry_key.contains('/');
        let first_party = first_party
            || (prefix.is_some_and(|p| normalize_provider(p) == normalized_provider)
                && inferred_creator(&bare_id) == Some(normalized_provider.as_str()));
        listings.push(ReferenceListing {
            source: SOURCE_ID,
            provider,
            normalized_provider,
            model_id: entry_key.clone(),
            bare_id,
            first_party,
            rates: parse_rates(map),
        });
    }
    SourceCatalog::build(listings)
}
