use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const CNY_PER_USD: f64 = 7.0;
pub const MAX_USD_RATE: f64 = 1_000_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Currency {
    #[serde(rename = "USD")]
    Usd,
    #[serde(rename = "CNY")]
    Cny,
}

impl Default for Currency {
    fn default() -> Self {
        Self::Usd
    }
}

impl Currency {
    pub fn to_usd(self, amount: f64) -> f64 {
        match self {
            Self::Usd => amount,
            Self::Cny => amount / CNY_PER_USD,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRates {
    pub input: f64,
    pub output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

impl TokenRates {
    fn in_usd(self, currency: Currency) -> Self {
        let usd = |amount| currency.to_usd(amount);
        Self {
            input: usd(self.input),
            output: usd(self.output),
            cache_read: self.cache_read.map(usd),
            cache_write: self.cache_write.map(usd),
        }
    }

    fn validate(&self) -> bool {
        valid_nonneg_rate(self.input)
            && valid_nonneg_rate(self.output)
            && self.cache_read.is_none_or(valid_nonneg_rate)
            && self.cache_write.is_none_or(valid_nonneg_rate)
    }

    fn approx_eq(&self, other: &Self) -> bool {
        approx_f64(self.input, other.input)
            && approx_f64(self.output, other.output)
            && approx_opt_f64(self.cache_read, other.cache_read)
            && approx_opt_f64(self.cache_write, other.cache_write)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRates {
    #[serde(flatten)]
    pub tokens: TokenRates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_12h: Option<f64>,
}

impl QuoteRates {
    fn validate(&self) -> bool {
        self.tokens.validate()
            && self.cache_write_1h.is_none_or(valid_nonneg_rate)
            && self.cache_write_12h.is_none_or(valid_nonneg_rate)
    }

    fn ttl_eq(&self, other: &Self) -> bool {
        approx_opt_f64(self.cache_write_1h, other.cache_write_1h)
            && approx_opt_f64(self.cache_write_12h, other.cache_write_12h)
    }

    pub(super) fn fill_missing_ttls(&mut self, other: Self) {
        self.cache_write_1h = self.cache_write_1h.or(other.cache_write_1h);
        self.cache_write_12h = self.cache_write_12h.or(other.cache_write_12h);
    }

    fn keep_common_ttls(&mut self, other: Self) {
        if !approx_opt_f64(self.cache_write_1h, other.cache_write_1h) {
            self.cache_write_1h = None;
        }
        if !approx_opt_f64(self.cache_write_12h, other.cache_write_12h) {
            self.cache_write_12h = None;
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRates<R> {
    pub threshold: u64,
    #[serde(flatten)]
    pub rates: R,
}

pub type LongContextRates = ContextRates<QuoteRates>;
pub type CustomLongContextRates = ContextRates<TokenRates>;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierRates {
    #[serde(flatten)]
    pub base: QuoteRates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_context: Option<LongContextRates>,
}

impl TierRates {
    pub(super) fn validate(&self) -> bool {
        self.base.validate()
            && self.long_context.as_ref().is_none_or(|long| long.threshold > 0 && long.rates.validate())
    }

    fn base_eq(&self, other: &Self) -> bool {
        self.base.tokens.approx_eq(&other.base.tokens)
            && match (&self.long_context, &other.long_context) {
                (Some(a), Some(b)) => a.threshold == b.threshold && a.rates.tokens.approx_eq(&b.rates.tokens),
                (None, None) => true,
                _ => false,
            }
    }

    fn approx_eq(&self, other: &Self) -> bool {
        self.base_eq(other)
            && self.base.ttl_eq(&other.base)
            && self.long_context.as_ref().zip(other.long_context.as_ref())
                .is_none_or(|(a, b)| a.rates.ttl_eq(&b.rates))
    }

    fn keep_common_ttls(&mut self, other: &Self) {
        self.base.keep_common_ttls(other.base);
        if let (Some(long), Some(other)) = (&mut self.long_context, &other.long_context) {
            long.rates.keep_common_ttls(other.rates);
        }
    }

    pub(super) fn quotes(&mut self) -> impl Iterator<Item = &mut QuoteRates> {
        std::iter::once(&mut self.base).chain(self.long_context.iter_mut().map(|long| &mut long.rates))
    }
}

/// 标准档和服务档位的单价，单位为美元 / 百万 Token。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    #[serde(flatten)]
    pub standard: TierRates,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub service_tiers: BTreeMap<String, TierRates>,
}

impl Rates {
    pub fn validate_reference(&self) -> Result<(), String> {
        if !self.standard.validate() {
            return Err("参考价缺少有效的标准文本费率".into());
        }
        if !self.service_tiers.values().all(TierRates::validate) {
            return Err("参考价特殊档位费率无效".into());
        }
        Ok(())
    }

    pub fn standard_eq(&self, other: &Self) -> bool {
        self.standard.base_eq(&other.standard)
    }

    pub fn approx_eq(&self, other: &Self) -> bool {
        self.standard.approx_eq(&other.standard)
            && self.service_tiers.len() == other.service_tiers.len()
            && self.service_tiers.iter().all(|(name, tier)| {
                other.service_tiers.get(name).is_some_and(|other| tier.approx_eq(other))
            })
    }

    /// 同名报价存在多个匹配时，额外费率只有各来源一致才采用。
    pub fn keep_common_variants(&mut self, other: &Self) {
        self.standard.keep_common_ttls(&other.standard);
        self.service_tiers.retain(|name, tier| {
            let Some(other) = other.service_tiers.get(name).filter(|other| tier.base_eq(other)) else {
                return false;
            };
            tier.keep_common_ttls(other);
            true
        });
    }
}

pub fn valid_nonneg_rate(value: f64) -> bool {
    value.is_finite() && (0.0..=MAX_USD_RATE).contains(&value)
}

pub fn approx_f64(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

pub fn approx_opt_f64(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => approx_f64(x, y),
        (None, None) => true,
        _ => false,
    }
}

/// 将 USD/token 转换为 USD/1M tokens，消除二进制浮点尾数噪点。
pub fn per_token_to_per_million(value: f64) -> Option<f64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let scaled = value * 1_000_000.0;
    if !scaled.is_finite() || scaled > MAX_USD_RATE {
        return None;
    }
    Some((scaled * 1e9).round() / 1e9)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomPriceInput {
    pub model: String,
    pub currency: Currency,
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_context: Option<CustomLongContextRates>,
}

/// 持久化保存的自定义价，保留用户选定币种与原始金额。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomPriceRecord {
    pub model: String,
    pub currency: Currency,
    pub input: f64,
    pub output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_context: Option<CustomLongContextRates>,
    pub updated_at: i64,
}

impl CustomPriceRecord {
    pub fn from_input(input: CustomPriceInput, now: i64) -> Result<Self, String> {
        let model = input.model.trim();
        if model.is_empty() || model.len() > 512 {
            return Err("请输入有效的模型 ID".into());
        }
        let max_amount = match input.currency {
            Currency::Usd => MAX_USD_RATE,
            Currency::Cny => MAX_USD_RATE * CNY_PER_USD,
        };
        let check = |value: f64, label: &str| -> Result<(), String> {
            if !value.is_finite() || value < 0.0 || value > max_amount {
                return Err(format!("{label}价格必须在 0 到 {max_amount:.0} 之间"));
            }
            Ok(())
        };
        check(input.input, "输入")?;
        check(input.output, "输出")?;
        if let Some(value) = input.cache_read {
            check(value, "缓存读取")?;
        }
        if let Some(value) = input.cache_write {
            check(value, "缓存写入")?;
        }
        if let Some(long) = &input.long_context {
            if long.threshold == 0 || long.threshold > 9_007_199_254_740_991 {
                return Err("请输入有效的长上下文阈值".into());
            }
            check(long.rates.input, "长上下文输入")?;
            check(long.rates.output, "长上下文输出")?;
            if let Some(value) = long.rates.cache_read {
                check(value, "长上下文缓存读取")?;
            }
            if let Some(value) = long.rates.cache_write {
                check(value, "长上下文缓存写入")?;
            }
        }
        Ok(Self {
            model: model.to_owned(),
            currency: input.currency,
            input: input.input,
            output: input.output,
            cache_read: input.cache_read,
            cache_write: input.cache_write,
            long_context: input.long_context,
            updated_at: now,
        })
    }

    pub fn to_usd_rates(&self) -> Rates {
        Rates {
            standard: TierRates {
                base: QuoteRates {
                    tokens: TokenRates {
                        input: self.input,
                        output: self.output,
                        cache_read: self.cache_read,
                        cache_write: self.cache_write,
                    }.in_usd(self.currency),
                    ..Default::default()
                },
                long_context: self.long_context.as_ref().map(|long| LongContextRates {
                    threshold: long.threshold,
                    rates: QuoteRates {
                        tokens: long.rates.in_usd(self.currency),
                        ..Default::default()
                    },
                }),
            },
            ..Default::default()
        }
    }

    pub fn original_rates(&self) -> CustomRatesView {
        CustomRatesView {
            input: self.input,
            output: self.output,
            cache_read: self.cache_read,
            cache_write: self.cache_write,
            long_context: self.long_context.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomRatesView {
    pub input: f64,
    pub output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_context: Option<CustomLongContextRates>,
}

#[derive(Clone, Debug)]
pub struct ReferenceListing {
    pub source: &'static str,
    pub provider: String,
    pub normalized_provider: String,
    pub model_id: String,
    pub bare_id: String,
    pub first_party: bool,
    pub rates: Option<Rates>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSummary {
    pub source: String,
    pub provider: String,
    pub model_id: String,
    pub rates: Rates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_as: Option<String>,
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPrice {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_as: Option<String>,
    pub currency: Currency,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_rates: Option<CustomRatesView>,
    pub rates: Rates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides_reference: Option<ReferenceSummary>,
    #[serde(skip)]
    pub(super) litellm_tiers: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventCost {
    pub total_usd: f64,
    pub source: String,
    pub currency: Currency,
    pub input_usd: f64,
    pub cache_read_usd: f64,
    pub cache_write_usd: f64,
    pub output_usd: f64,
    pub input_rate: f64,
    pub output_rate: f64,
    pub cache_read_rate: f64,
    pub cache_write_rate: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_rate: Option<f64>,
    pub long_context: bool,
    /// 按非标准服务档位计价时的档位名（priority/flex/batch）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// 目录未给出该档位价格时采用的已公开倍率规则（目前仅 Codex priority ×2）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiplier: Option<f64>,
}

/// 无法给出费用的原因；未知费用绝不按 0 计入。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingIssue {
    /// 输入或输出 Token 未记录，或只有下限。
    UsageIncomplete,
    /// 日志只给出总量，无法拆分为输入/输出。
    Unclassified,
    /// 缓存是否包含在输入中未确认（Qoder）。
    CacheAmbiguous,
    /// 模型没有自定义价，参考目录也没有可用标准价。
    Unpriced,
    /// 请求使用了目录未定价的服务档位（如 flex、Claude 快速模式）。
    UnsupportedTier,
    /// 请求含有未定价的分项（如无价可依的 1 小时缓存写入）。
    UnsupportedComponent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub id: String,
    pub name: String,
    pub url: String,
    pub fetched_at: Option<i64>,
    pub model_count: usize,
    pub priced_count: usize,
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingCatalogStatus {
    pub revision: u64,
    pub sources: Vec<SourceStatus>,
    pub custom_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_error: Option<String>,
    pub exchange_rate_cny_per_usd: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPriceRow {
    pub model: String,
    /// 单一提供商的用量行。合并多个来源的用量时留空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub price: Option<ResolvedPrice>,
}

/// 统计和请求分页共用的计费数据视图。
pub struct BillableEvent<'a> {
    pub agent: &'a str,
    pub time: i64,
    pub model: &'a str,
    pub tokens: &'a crate::usage::types::Tokens,
    pub details: &'a crate::usage::types::RecordedFields,
    pub provider: Option<&'a str>,
    pub tier: Option<&'a str>,
    pub failed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricesResponse {
    pub models: Vec<ModelPriceRow>,
    pub catalog: PricingCatalogStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceSearchItem {
    pub source: String,
    pub provider: String,
    pub model_id: String,
    pub rates: Rates,
}
