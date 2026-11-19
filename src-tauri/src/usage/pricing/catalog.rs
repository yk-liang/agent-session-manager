//! 公共参考价目录索引与跨提供商安全匹配。
use super::types::{Rates, ReferenceListing, ReferenceSearchItem};
use std::collections::{HashMap, HashSet};

pub fn model_key(model: &str) -> String {
    model.trim().to_ascii_lowercase()
}

/// 拆分模型名中的显式 `provider/model` 前缀；仅当包含 `/` 时返回前缀。
pub fn split_provider_prefix(model: &str) -> (Option<&str>, &str) {
    let trimmed = model.trim();
    match trimmed.split_once('/') {
        Some((prefix, rest)) if !prefix.is_empty() && !rest.is_empty() => (Some(prefix), rest),
        _ => (None, trimmed),
    }
}

pub fn bare_model_key(model: &str) -> String {
    let key = model_key(model);
    match key.rsplit_once('/') {
        Some((_, tail)) if !tail.is_empty() => tail.to_owned(),
        _ => key,
    }
}

/// 仅对明确等价的官方提供商别名做归一化；未知/自定义提供商保持原名，避免跨商错配。
pub fn normalize_provider(provider: &str) -> String {
    let key = provider.trim().to_ascii_lowercase();
    match key.as_str() {
        "google" | "gemini" | "google-ai" | "google-ai-studio" | "google_ai_studio" => {
            "google".into()
        }
        "vertex"
        | "vertex_ai"
        | "vertex-ai"
        | "google-vertex"
        | "google-vertex-anthropic"
        | "vertex_ai-language-models"
        | "vertex_ai-chat-models"
        | "vertex_ai-anthropic_models"
        | "vertex_ai-llama_models"
        | "vertex_ai-mistral_models"
        | "vertex_ai-ai21_models"
        | "vertex_ai-vision-models" => "vertex_ai".into(),
        "bedrock" | "amazon-bedrock" | "aws-bedrock" | "bedrock_converse" => "bedrock".into(),
        "openai" | "text-completion-openai" | "openai-chat" => "openai".into(),
        "anthropic" => "anthropic".into(),
        "deepseek" => "deepseek".into(),
        "xai" | "x-ai" => "xai".into(),
        "alibaba" | "alibaba-cn" | "dashscope" | "qwen" => "alibaba".into(),
        "zhipuai" | "zhipu" | "zai" | "bigmodel" => "zhipuai".into(),
        "moonshotai" | "moonshot" => "moonshotai".into(),
        "minimax" => "minimax".into(),
        "mistral" | "mistralai" => "mistral".into(),
        "cohere" | "cohere_chat" => "cohere".into(),
        "openrouter" => "openrouter".into(),
        "azure" | "azure_ai" | "azure-openai" | "azure_text" => "azure".into(),
        "groq" => "groq".into(),
        "together" | "together_ai" | "togetherai" => "together".into(),
        "fireworks" | "fireworks_ai" | "fireworks-ai" => "fireworks".into(),
        "perplexity" => "perplexity".into(),
        "github-copilot" | "github_copilot" => "github-copilot".into(),
        _ => key,
    }
}

/// 根据无前缀模型家族名判断原厂提供商，用于区分原厂报价与第三方转售/免费代理。
pub fn inferred_creator(bare_model: &str) -> Option<&'static str> {
    let m = bare_model.trim().to_ascii_lowercase();
    if m.starts_with("claude-") {
        Some("anthropic")
    } else if m.starts_with("gpt-")
        || m.starts_with("chatgpt-")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
        || m.starts_with("codex-")
    {
        Some("openai")
    } else if m.starts_with("gemini-") || m.starts_with("gemma-") {
        Some("google")
    } else if m.starts_with("deepseek-") {
        Some("deepseek")
    } else if m.starts_with("grok-") {
        Some("xai")
    } else if m.starts_with("qwen") || m.starts_with("qwq-") {
        Some("alibaba")
    } else if m.starts_with("glm-") {
        Some("zhipuai")
    } else if m.starts_with("kimi-") || m.starts_with("moonshot-") {
        Some("moonshotai")
    } else if m.starts_with("minimax-") {
        Some("minimax")
    } else if m.starts_with("mistral-")
        || m.starts_with("codestral-")
        || m.starts_with("devstral-")
        || m.starts_with("pixtral-")
    {
        Some("mistral")
    } else {
        None
    }
}

pub enum MatchOutcome {
    Priced {
        provider: String,
        model_id: String,
        rates: Rates,
    },
    /// 目录中存在该模型，但价格冲突、缺失或计费单位不受支持；禁止退回后缀兜底。
    ListedUnpriced,
    Absent,
}

#[derive(Clone, Debug)]
pub struct SourceCatalog {
    pub listings: Vec<ReferenceListing>,
    pub priced_count: usize,
    providers: HashSet<String>,
    by_provider_model: HashMap<(String, String), Vec<usize>>,
    by_provider_bare: HashMap<(String, String), Vec<usize>>,
    by_bare: HashMap<String, Vec<usize>>,
    by_first_party: HashMap<String, Vec<usize>>,
}

impl SourceCatalog {
    pub fn build(mut listings: Vec<ReferenceListing>) -> Result<Self, String> {
        let priced_count = listings.iter().filter(|item| item.rates.is_some()).count();
        if priced_count == 0 {
            return Err("参考价目录中没有可用的文本 Token 费率".into());
        }
        listings.sort_by(|a, b| {
            a.provider
                .cmp(&b.provider)
                .then_with(|| a.model_id.cmp(&b.model_id))
        });

        let mut providers = HashSet::new();
        let mut by_provider_model: HashMap<(String, String), Vec<usize>> = HashMap::new();
        let mut by_provider_bare: HashMap<(String, String), Vec<usize>> = HashMap::new();
        let mut by_bare: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_first_party: HashMap<String, Vec<usize>> = HashMap::new();

        for (index, item) in listings.iter().enumerate() {
            providers.insert(item.normalized_provider.clone());
            let norm_model = model_key(&item.model_id);
            by_provider_model
                .entry((item.normalized_provider.clone(), norm_model))
                .or_default()
                .push(index);
            by_provider_bare
                .entry((item.normalized_provider.clone(), item.bare_id.clone()))
                .or_default()
                .push(index);
            by_bare.entry(item.bare_id.clone()).or_default().push(index);
            if item.first_party {
                by_first_party
                    .entry(item.bare_id.clone())
                    .or_default()
                    .push(index);
            }
        }

        Ok(Self {
            listings,
            priced_count,
            providers,
            by_provider_model,
            by_provider_bare,
            by_bare,
            by_first_party,
        })
    }

    pub fn has_provider(&self, normalized: &str) -> bool {
        self.providers.contains(normalized)
    }

    pub fn match_first_party(&self, model: &str) -> MatchOutcome {
        self.by_first_party
            .get(model)
            .map_or(MatchOutcome::Absent, |indices| {
                self.select_unanimous(indices)
            })
    }

    pub fn match_model(&self, model: &str, provider_hint: Option<&str>) -> MatchOutcome {
        let key = model_key(model);
        if key.is_empty() || key == "unknown" {
            return MatchOutcome::Absent;
        }
        let (explicit_prefix, rest) = split_provider_prefix(&key);
        if let Some(prefix) = explicit_prefix {
            let target_provider = normalize_provider(prefix);
            // 1. 优先匹配同一提供商下的完整模型 ID（例如 openrouter/anthropic/claude-sonnet-4）
            if let Some(indices) = self
                .by_provider_model
                .get(&(target_provider.clone(), rest.to_owned()))
            {
                return self.select_unanimous(indices);
            }
            // 2. 兼容目录中把完整 `provider/model` 存成 model_id 的情况（如 LiteLLM 键名）
            if let Some(indices) = self
                .by_provider_model
                .get(&(target_provider.clone(), key.clone()))
            {
                return self.select_unanimous(indices);
            }
            // 3. 仅在不含二级路由前缀时匹配同提供商的裸 ID
            if !rest.contains('/') {
                if let Some(indices) = self
                    .by_provider_bare
                    .get(&(target_provider, rest.to_owned()))
                {
                    return self.select_unanimous(indices);
                }
            }
            // 显式指定了提供商但目录中无该提供商条目：严禁跨提供商套用其他厂商报价。
            return MatchOutcome::Absent;
        }

        // 无前缀裸模型名：若事件带有提供商提示，先按该提供商精确匹配；非原厂/自定义提供商不跨商兜底。
        if let Some(hint) = provider_hint
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(normalize_provider)
        {
            if let Some(indices) = self.by_provider_model.get(&(hint.clone(), key.clone())) {
                return self.select_unanimous(indices);
            }
            if let Some(indices) = self.by_provider_bare.get(&(hint.clone(), key.clone())) {
                return self.select_unanimous(indices);
            }
            if inferred_creator(&key).is_none_or(|creator| creator != hint) {
                return MatchOutcome::Absent;
            }
        }

        // 原厂报价优先
        if let Some(indices) = self.by_first_party.get(&key) {
            return self.select_unanimous(indices);
        }
        // 无原厂报价时，仅在所有包含该模型名的提供商定价一致时采用，价格冲突视为未定价。
        if let Some(indices) = self.by_bare.get(&key) {
            return self.select_unanimous(indices);
        }
        MatchOutcome::Absent
    }

    fn select_unanimous(&self, indices: &[usize]) -> MatchOutcome {
        if indices.is_empty() {
            return MatchOutcome::Absent;
        }
        let first = &self.listings[indices[0]];
        let Some(first_rates) = &first.rates else {
            return MatchOutcome::ListedUnpriced;
        };
        let mut rates = first_rates.clone();
        for &index in &indices[1..] {
            match &self.listings[index].rates {
                // 同名条目（如 `gpt-x` 与 `openai/gpt-x`）只在特殊档位上不同时，标准价仍然可用。
                Some(other) if other.standard_eq(first_rates) => rates.keep_common_variants(other),
                _ => return MatchOutcome::ListedUnpriced,
            }
        }
        MatchOutcome::Priced {
            provider: first.provider.clone(),
            model_id: first.model_id.clone(),
            rates,
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<ReferenceSearchItem> {
        let words: Vec<String> = query
            .split_whitespace()
            .map(|w| w.to_ascii_lowercase())
            .collect();
        let exact = words.join(" ");
        let mut matched: Vec<(&ReferenceListing, i32)> = self
            .listings
            .iter()
            .filter_map(|item| {
                let rates = item.rates.as_ref()?;
                let haystack = format!("{}/{}", item.provider, item.model_id).to_ascii_lowercase();
                if !words.iter().all(|w| haystack.contains(w)) {
                    return None;
                }
                let mut score = 0;
                if !exact.is_empty() && item.bare_id == exact {
                    score -= 4;
                } else if !exact.is_empty() && model_key(&item.model_id) == exact {
                    score -= 3;
                }
                if item.first_party {
                    score -= 2;
                }
                let _ = rates;
                Some((item, score))
            })
            .collect();
        matched.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| a.0.provider.cmp(&b.0.provider))
                .then_with(|| a.0.model_id.cmp(&b.0.model_id))
        });
        matched
            .into_iter()
            .take(limit)
            .filter_map(|(item, _)| {
                Some(ReferenceSearchItem {
                    source: item.source.to_owned(),
                    provider: item.provider.clone(),
                    model_id: item.model_id.clone(),
                    rates: item.rates.clone()?,
                })
            })
            .collect()
    }
}
