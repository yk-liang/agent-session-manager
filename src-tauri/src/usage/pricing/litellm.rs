//! LiteLLM Model Cost Map 解析器；公开目录单位为 USD / Token，规范换算为 USD / 1M tokens。
use super::{
    catalog::{
        bare_model_key, inferred_creator, normalize_provider, split_provider_prefix, SourceCatalog,
    },
    types::{
        approx_f64, per_token_to_per_million, LongContextRates, Rates, ReferenceListing, TierRates,
    },
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const SOURCE_ID: &str = "litellm";
pub const SOURCE_NAME: &str = "LiteLLM";

const UNSUPPORTED_UNIT_FIELDS: &[&str] = &[
    "input_cost_per_character",
    "output_cost_per_character",
    "input_cost_per_second",
    "output_cost_per_second",
    "input_cost_per_audio_second",
    "output_cost_per_audio_second",
    "input_cost_per_image",
    "output_cost_per_image",
    "input_cost_per_pixel",
    "output_cost_per_pixel",
    "cost_per_query",
    "input_cost_per_request",
    "output_cost_per_request",
    "search_context_cost_per_query",
];

fn opt_f64(map: &Map<String, Value>, key: &str) -> Option<f64> {
    map.get(key).and_then(Value::as_f64)
}

fn has_unsupported_unit(map: &Map<String, Value>) -> bool {
    if UNSUPPORTED_UNIT_FIELDS
        .iter()
        .any(|key| opt_f64(map, key).is_some_and(|v| !v.is_finite() || v > 0.0))
    {
        return true;
    }
    // 存在多档 tiered_pricing 数组时无法用单阈值准确表达，拒绝降级为普通费率。
    if map
        .get("tiered_pricing")
        .and_then(Value::as_array)
        .is_some_and(|tiers| !tiers.is_empty())
    {
        return true;
    }
    false
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
}

#[derive(Default)]
struct Fields {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

impl Fields {
    fn set(&mut self, field: Field, rate: f64) {
        *match field {
            Field::Input => &mut self.input,
            Field::Output => &mut self.output,
            Field::CacheRead => &mut self.cache_read,
            Field::CacheWrite => &mut self.cache_write,
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
    /// 只能表达单一阈值；多阈值或长上下文缺输入/输出时返回 None。
    fn rates(self) -> Option<TierRates> {
        let long_context = match self.above.len() {
            0 => None,
            1 => {
                let (threshold, tier) = self.above.into_iter().next()?;
                Some(LongContextRates {
                    threshold,
                    input: tier.input?,
                    output: tier.output?,
                    cache_read: tier.cache_read,
                    cache_write: tier.cache_write,
                })
            }
            _ => return None,
        };
        Some(TierRates {
            input: self.base.input?,
            output: self.base.output?,
            cache_read: self.base.cache_read,
            cache_write: self.base.cache_write,
            long_context,
        })
    }
}

const FIELD_PREFIXES: [(&str, Field); 4] = [
    ("input_cost_per_token", Field::Input),
    ("output_cost_per_token", Field::Output),
    ("cache_read_input_token_cost", Field::CacheRead),
    ("cache_creation_input_token_cost", Field::CacheWrite),
];
const SERVICE_SUFFIXES: [(&str, &str); 3] = [
    ("_priority", "priority"),
    ("_flex", "flex"),
    ("_batches", "batch"),
];

/// 归类条目中的全部文本 Token 费率键。返回 Err 表示含有无法准确表达的阈值费率，
/// 此时整个条目按未定价处理，避免静默降级为普通费率。
/// - `_priority` / `_flex` / `_batches` 后缀归入对应服务档位，不影响标准价；
/// - `cache_creation_input_token_cost_above_1hr` 记为 1 小时缓存写入价；
///   其他 TTL（如 OpenAI 的 `_above_12hr`）没有任何来源记录对应 Token，忽略且不会套用。
fn collect_tiers(
    map: &Map<String, Value>,
) -> Result<(BTreeMap<&'static str, TierFields>, Option<f64>), ()> {
    let mut tiers: BTreeMap<&'static str, TierFields> = BTreeMap::new();
    let mut cache_write_1h = None;
    for (key, value) in map {
        let Some(raw) = value.as_f64() else { continue };
        let (stem, service) = SERVICE_SUFFIXES
            .iter()
            .find_map(|(suffix, name)| key.strip_suffix(suffix).map(|stem| (stem, *name)))
            .unwrap_or((key.as_str(), "standard"));
        let Some((field, rest)) = FIELD_PREFIXES
            .iter()
            .find_map(|(prefix, field)| stem.strip_prefix(prefix).map(|rest| (*field, rest)))
            .filter(|(_, rest)| rest.is_empty() || rest.starts_with('_'))
        else {
            // 其他计费单位由 has_unsupported_unit 处理；未知的阈值字段不能静默忽略。
            if key.contains("_above_") && key.contains("cost") {
                return Err(());
            }
            continue;
        };
        let rate = per_token_to_per_million(raw).ok_or(())?;
        let tier = tiers.entry(service).or_default();
        if rest.is_empty() {
            tier.base.set(field, rate);
            continue;
        }
        let Some(suffix) = rest.strip_prefix("_above_") else {
            // 例如 `_cache_hit` 等别名字段：不属于本项目使用的文本价，忽略。
            continue;
        };
        if let Some(threshold) = parse_token_threshold(suffix) {
            tier.above.entry(threshold).or_default().set(field, rate);
        } else if suffix.ends_with("hr") || suffix.contains("hr_above_") {
            if matches!(field, Field::CacheWrite) && service == "standard" && suffix == "1hr" {
                cache_write_1h = Some(rate);
            }
        } else {
            return Err(());
        }
    }
    Ok((tiers, cache_write_1h))
}

fn parse_rates(map: &Map<String, Value>) -> Option<Rates> {
    if !supported_mode(map) || has_unsupported_unit(map) {
        return None;
    }
    if let (Some(reasoning), Some(output)) = (
        opt_f64(map, "output_cost_per_reasoning_token"),
        opt_f64(map, "output_cost_per_token"),
    ) {
        if !approx_f64(reasoning, output) {
            return None;
        }
    }
    let (mut tiers, cache_write_1h) = collect_tiers(map).ok()?;
    let standard = tiers.remove("standard")?.rates()?;
    let rates = Rates {
        input: standard.input,
        output: standard.output,
        cache_read: standard.cache_read,
        cache_write: standard.cache_write,
        long_context: standard.long_context,
        cache_write_1h,
        // 缺输入/输出或多阈值的特殊档位只丢弃该档位，标准价保持可用。
        service_tiers: tiers
            .into_iter()
            .filter_map(|(name, tier)| Some((name.to_owned(), tier.rates()?)))
            .collect(),
    };
    rates.validate_reference().ok()?;
    Some(rates)
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
