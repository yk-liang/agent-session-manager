use super::proto::Message;
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Model {
    pub id: Option<u64>,
    pub name: Option<String>,
    pub display: Option<String>,
}

fn known_model(id: u64) -> Option<(&'static str, Option<&'static str>)> {
    Some(match id {
        246 => ("gemini-2.5-pro", None),
        281 | 282 => ("claude-sonnet-4", None),
        290 | 291 => ("claude-opus-4", None),
        312 => ("gemini-2.5-flash", None),
        313 | 329 => ("gemini-2.5-flash-thinking", None),
        330 => ("gemini-2.5-flash-lite", None),
        333 | 334 => ("claude-sonnet-4-5", None),
        340 | 341 => ("claude-haiku-4-5", None),
        342 => ("gpt-oss-120b", Some("medium")),
        1016 | 1036 | 1037 => ("gemini-3.1-pro", None),
        1018 | 1047 | 1084 => ("gemini-3-flash-preview", None),
        1020 => ("gemini-3.5-flash", Some("medium")),
        1026 => ("claude-opus-4-6", None),
        1035 => ("claude-sonnet-4-6", None),
        1050 => ("gemini-3.1-flash-lite", None),
        1071..=1073 => ("gemini-3.6-flash", Some(["high", "medium", "low"][(id - 1071) as usize])),
        1132 | 1133 => ("gemini-3.5-flash", Some("high")),
        1187 => ("gemini-3.5-flash", Some("extra-low")),
        1298..=1300 => ("gemini-3.7-flash", Some(["high", "medium", "low"][(id - 1298) as usize])),
        1318..=1320 => ("gemini-3.8-flash", Some(["high", "medium", "low"][(id - 1318) as usize])),
        1402 => ("claude-opus-5-5", Some("high")),
        1403 => ("claude-sonnet-5-5", Some("low")),
        1405 => ("claude-sonnet-5-5", Some("high")),
        1417 => ("gemini-4-argon", Some("high")),
        1418..=1419 => ("gemini-4-argon", None),
        1526 => ("gemini-4-hydrogen", Some("medium")),
        1528 => ("gemini-4-hydrogen", Some("xhigh")),
        1532..=1535 => ("gemini-4-neon", Some(["high", "medium", "low", "xhigh"][(id - 1532) as usize])),
        _ => return None,
    })
}

fn placeholder_id(name: &str) -> Option<u64> {
    name.to_ascii_lowercase()
        .strip_prefix("model_placeholder_m")?
        .parse::<u64>().ok()?.checked_add(1000)
}

fn model_parts(value: &str) -> (&str, Option<&'static str>) {
    let Some((model, suffix)) = value.strip_suffix(')')
        .and_then(|name| name.rsplit_once(" ("))
        .or_else(|| value.rsplit_once('-'))
    else {
        return (value, None);
    };
    let effort = match suffix.to_ascii_lowercase().as_str() {
        "minimal" => "minimal",
        "low" if model.ends_with("-extra") => {
            return (model.trim_end_matches("-extra"), Some("extra-low"));
        }
        "extra-low" => "extra-low",
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        "xhigh" => "xhigh",
        "max" => "max",
        _ => return (value, None),
    };
    (model, Some(effort))
}

impl Model {
    pub(super) fn is_recorded(&self) -> bool {
        self.id.is_some() || self.name.is_some() || self.display.is_some()
    }

    pub(crate) fn generation(request: &Message<'_>) -> Self {
        Self {
            id: request.number(3).filter(|&id| id != 0),
            name: request.text(19).map(str::to_owned),
            display: request.text(21).map(str::to_owned),
        }
    }

    pub(crate) fn step(metadata: &Message<'_>) -> std::io::Result<Self> {
        let info = metadata.message(24)?;
        Ok(Self {
            id: info
                .number(1)
                .or_else(|| metadata.number(11))
                .filter(|&id| id != 0),
            name: info.text(12).or_else(|| info.text(8)).map(str::to_owned),
            display: None,
        })
    }

    pub(crate) fn remember(&self, models: &mut HashMap<u64, Model>) {
        if let Some(id) = self
            .id
            .filter(|_| self.name.is_some() || self.display.is_some())
        {
            let entry = models.entry(id).or_default();
            if let Some(name) = &self.name {
                if entry.name.is_none() || placeholder_id(name).is_none() {
                    entry.name = Some(name.clone());
                }
            }
            if self.display.is_some() {
                entry.display = self.display.clone();
            }
        }
    }

    pub(crate) fn resolve(
        &self,
        usage_id: Option<u64>,
        models: &HashMap<u64, Model>,
    ) -> (String, Option<String>) {
        let id = usage_id.filter(|&id| id != 0).or(self.id);
        let context = if self.id.is_some() && self.id != id {
            None
        } else {
            Some(self)
        };
        let observed = id.and_then(|id| models.get(&id));
        let display = context
            .and_then(|m| m.display.as_deref())
            .or_else(|| observed.and_then(|m| m.display.as_deref()));
        let names = [context, observed].into_iter().flatten()
            .filter_map(|m| m.name.as_deref());
        let raw = names.clone().find(|name| placeholder_id(name).is_none());
        let id = id.or_else(|| names.filter_map(placeholder_id).next());
        let known = id.and_then(known_model);
        let raw_id = raw.map(|name| match name.to_ascii_lowercase().as_str() {
            "gemini-pro-default" | "gemini-pro-agent" => "gemini-3.1-pro",
            "gemini-3-flash-a" | "gemini-3-flash-b" | "gemini-3-flash-agent"
            | "gemini-3-flash-agent-a" | "gemini-3-flash-agent-b" => "gemini-3.5-flash-high",
            "gemini-3-flash-c" | "gemini-3-flash" => "gemini-3-flash-preview",
            "model_openai_gpt_oss_120b_medium" => "gpt-oss-120b-medium",
            _ => name,
        });
        let name = known.map(|(name, _)| name.to_owned())
            .or_else(|| raw_id.map(|name| model_parts(name).0.to_owned()))
            .or_else(|| display.map(|display| {
                let name = model_parts(display).0.to_lowercase()
                    .split_whitespace().collect::<Vec<_>>().join("-");
                if name.starts_with("claude-") { name.replace('.', "-") } else { name }
            }))
            .or_else(|| id.map(|id| format!("antigravity-model-{id}")))
            .unwrap_or_else(|| "unknown".into());
        let effort = display.and_then(|name| model_parts(name).1)
            .or_else(|| raw_id.and_then(|name| model_parts(name).1))
            .or_else(|| known.and_then(|(_, effort)| effort))
            .map(str::to_owned);
        (name, effort)
    }
}
