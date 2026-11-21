//! Models.dev (https://models.dev/api.json) 解析器；原始报价单位为 USD / 1M tokens。
//! 逐个模型宽松解析：单个模型或档位的结构变化只影响该模型，不会让整个提供商消失。
use super::{
    catalog::{bare_model_key, inferred_creator, normalize_provider, SourceCatalog},
    types::{approx_f64, LongContextRates, QuoteRates, Rates, ReferenceListing, TierRates, TokenRates},
};
use serde_json::Value;

pub const SOURCE_ID: &str = "models.dev";
pub const SOURCE_NAME: &str = "Models.dev";

fn num(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn has_text_rates(rates: &Value) -> bool {
    num(rates, "input").is_some()
        && num(rates, "output").is_some_and(|output| {
            rates.get("reasoning").is_none_or(|value| {
                value.is_null() || value.as_f64().is_some_and(|rate| approx_f64(rate, output))
            })
        })
}

fn quote(rates: &Value) -> Option<QuoteRates> {
    if !has_text_rates(rates) {
        return None;
    }
    Some(QuoteRates {
        tokens: TokenRates {
            input: num(rates, "input")?,
            output: num(rates, "output")?,
            cache_read: num(rates, "cache_read"),
            cache_write: num(rates, "cache_write"),
        },
        ..Default::default()
    })
}

/// 只把 `type: "context"` 档位当作长上下文价；其他档位描述的是不同计价的 Token，
/// 不参与标准价，也不会使标准价失效。多个上下文档位无法用单阈值表达，按未定价处理。
pub(super) fn parse_rates(cost: &Value) -> Option<Rates> {
    let base = quote(cost)?;
    let context_tiers: Vec<&Value> = cost
        .get("tiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tier| tier["tier"]["type"].as_str() == Some("context"))
        .collect();
    let long_context = match context_tiers.as_slice() {
        [] => match cost.get("context_over_200k").filter(|v| v.is_object()) {
            Some(over) => Some(LongContextRates { threshold: 200_000, rates: quote(over)? }),
            None => None,
        },
        [tier] => {
            let size = tier["tier"]["size"]
                .as_f64()
                .filter(|s| s.fract() == 0.0 && *s > 0.0)?;
            Some(LongContextRates { threshold: size as u64, rates: quote(tier)? })
        }
        _ => return None,
    };
    let rates = Rates { standard: TierRates { base, long_context }, ..Default::default() };
    rates.validate_reference().ok()?;
    Some(rates)
}

pub fn parse(raw: &[u8]) -> Result<SourceCatalog, String> {
    let root: Value = serde_json::from_slice(raw)
        .map_err(|error| format!("解析 Models.dev 目录失败：{error}"))?;
    let object = root
        .as_object()
        .ok_or_else(|| "Models.dev 目录根节点必须是对象".to_owned())?;
    let mut listings = Vec::new();
    for (provider_key, provider) in object {
        let Some(models) = provider.get("models").and_then(Value::as_object) else {
            continue;
        };
        let provider_id = provider["id"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(provider_key)
            .to_owned();
        let normalized_provider = normalize_provider(&provider_id);
        for (entry_key, model) in models {
            let model_id = model["id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(entry_key)
                .to_owned();
            let bare_id = bare_model_key(&model_id);
            let canonical_creator = model["canonical_model_id"]
                .as_str()
                .and_then(|c| c.split_once('/').map(|(p, _)| normalize_provider(p)));
            let first_party = match canonical_creator {
                Some(creator) => creator == normalized_provider,
                None => inferred_creator(&bare_id) == Some(normalized_provider.as_str()),
            };
            listings.push(ReferenceListing {
                source: SOURCE_ID,
                provider: provider_id.clone(),
                normalized_provider: normalized_provider.clone(),
                model_id,
                bare_id,
                first_party,
                rates: model.get("cost").and_then(parse_rates),
            });
        }
    }
    SourceCatalog::build(listings)
}
