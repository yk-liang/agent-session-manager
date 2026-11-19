//! Models.dev (https://models.dev/api.json) 解析器；原始报价单位为 USD / 1M tokens。
//! 逐个模型宽松解析：单个模型或档位的结构变化只影响该模型，不会让整个提供商消失。
use super::{
    catalog::{bare_model_key, inferred_creator, normalize_provider, SourceCatalog},
    types::{approx_f64, LongContextRates, Rates, ReferenceListing},
};
use serde_json::Value;

pub const SOURCE_ID: &str = "models.dev";
pub const SOURCE_NAME: &str = "Models.dev";

fn num(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

/// 推理与音频单价和文本一致（或未单列）时，文本 Token 费率才能完整表达该价格。
fn text_only(rates: &Value) -> bool {
    let same = |special: &str, base: &str| match (num(rates, special), num(rates, base)) {
        (None, _) => rates.get(special).is_none_or(Value::is_null),
        (Some(s), Some(b)) => approx_f64(s, b),
        _ => false,
    };
    num(rates, "input").is_some()
        && num(rates, "output").is_some()
        && same("reasoning", "output")
        && same("input_audio", "input")
        && same("output_audio", "output")
}

fn long_context(rates: &Value, threshold: u64) -> Option<LongContextRates> {
    if threshold == 0 || !text_only(rates) {
        return None;
    }
    Some(LongContextRates {
        threshold,
        input: num(rates, "input")?,
        output: num(rates, "output")?,
        cache_read: num(rates, "cache_read"),
        cache_write: num(rates, "cache_write"),
    })
}

/// 只把 `type: "context"` 档位当作长上下文价；其他档位描述的是不同计价的 Token，
/// 不参与标准价，也不会使标准价失效。多个上下文档位无法用单阈值表达，按未定价处理。
pub(super) fn parse_rates(cost: &Value) -> Option<Rates> {
    if !cost.is_object() || !text_only(cost) {
        return None;
    }
    let mut rates = Rates {
        input: num(cost, "input")?,
        output: num(cost, "output")?,
        cache_read: num(cost, "cache_read"),
        cache_write: num(cost, "cache_write"),
        ..Default::default()
    };
    let context_tiers: Vec<&Value> = cost
        .get("tiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tier| tier["tier"]["type"].as_str() == Some("context"))
        .collect();
    match context_tiers.as_slice() {
        [] => {
            if let Some(over) = cost.get("context_over_200k").filter(|v| v.is_object()) {
                rates.long_context = Some(long_context(over, 200_000)?);
            }
        }
        [tier] => {
            let size = tier["tier"]["size"]
                .as_f64()
                .filter(|s| s.fract() == 0.0 && *s > 0.0)?;
            rates.long_context = Some(long_context(tier, size as u64)?);
        }
        _ => return None,
    }
    rates.validate_reference().ok()?;
    Some(rates)
}

pub fn parse(raw: &[u8]) -> Result<SourceCatalog, String> {
    let root: Value = serde_json::from_slice(raw)
        .map_err(|error| format!("解析 Models.dev 目录失败：{error}"))?;
    let object = root
        .as_object()
        .ok_or_else(|| "Models.dev 目录根节点必须是对象".to_owned())?;
    // api.json 顶层即 provider -> { models }; 同时兼容 catalog.json 的 { providers: { ... } }。
    let providers_map = match object.get("providers").and_then(Value::as_object) {
        Some(nested) if object.get("models").is_none() || object.len() <= 3 => nested,
        _ => object,
    };

    let mut listings = Vec::new();
    for (provider_key, provider) in providers_map {
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
