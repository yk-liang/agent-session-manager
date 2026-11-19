use super::proto::Message;
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Model {
    pub id: Option<u64>,
    pub name: Option<String>,
    pub display: Option<String>,
}

fn known_name(id: u64) -> Option<&'static str> {
    Some(match id {
        246 => "gemini-2.5-pro",
        281 | 282 => "claude-sonnet-4",
        290 | 291 => "claude-opus-4",
        312 => "gemini-2.5-flash",
        313 | 329 => "gemini-2.5-flash-thinking",
        330 => "gemini-2.5-flash-lite",
        333 | 334 => "claude-sonnet-4-5",
        340 | 341 => "claude-haiku-4-5",
        342 => "gpt-oss-120b-medium",
        1016 | 1036 | 1037 => "gemini-3.1-pro",
        1018 | 1047 | 1084 => "gemini-3-flash-preview",
        1020 => "gemini-3.5-flash-medium",
        1026 => "claude-opus-4-6",
        1035 => "claude-sonnet-4-6",
        1050 => "gemini-3.1-flash-lite",
        1071 => "gemini-3.6-flash-high",
        1072 => "gemini-3.6-flash-medium",
        1073 => "gemini-3.6-flash-low",
        1132 | 1133 => "gemini-3.5-flash-high",
        1187 => "gemini-3.5-flash-extra-low",
        1298 => "gemini-3.7-flash-high",
        1299 => "gemini-3.7-flash-medium",
        1300 => "gemini-3.7-flash-low",
        1318 => "gemini-3.8-flash-high",
        1319 => "gemini-3.8-flash-medium",
        1320 => "gemini-3.8-flash-low",
        1402 => "claude-opus-5-5",
        1417..=1419 => "gemini-4-argon",
        _ => return None,
    })
}

fn placeholder_id(name: &str) -> Option<u64> {
    name.to_ascii_lowercase()
        .strip_prefix("model_placeholder_m")?
        .parse::<u64>().ok()?.checked_add(1000)
}

fn effort(value: &str) -> Option<&str> {
    let suffix = value
        .rsplit_once(" (")
        .and_then(|(_, tail)| tail.strip_suffix(')'))
        .or_else(|| value.rsplit_once('-').map(|(_, tail)| tail))?;
    Some(match suffix.to_ascii_lowercase().as_str() {
        "minimal" => "minimal",
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        "xhigh" => "xhigh",
        "max" => "max",
        _ => return None,
    })
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
        let name = id
            .and_then(known_name)
            .map(str::to_owned)
            .or_else(|| {
                raw.and_then(|raw| {
                    let lower = raw.to_ascii_lowercase();
                    Some(
                        match lower.as_str() {
                            "gemini-pro-default" | "gemini-pro-agent" => "gemini-3.1-pro",
                            "gemini-3-flash-a"
                            | "gemini-3-flash-b"
                            | "gemini-3-flash-agent"
                            | "gemini-3-flash-agent-a"
                            | "gemini-3-flash-agent-b" => "gemini-3.5-flash-high",
                            "gemini-3-flash-c" | "gemini-3-flash" => "gemini-3-flash-preview",
                            "model_openai_gpt_oss_120b_medium" => "gpt-oss-120b-medium",
                            _ => raw,
                        }
                        .to_owned(),
                    )
                })
            })
            .or_else(|| {
                display.map(|name| {
                    let id = name.split(" (")
                        .next()
                        .unwrap_or(name)
                        .to_lowercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join("-");
                    if id.starts_with("claude-") {
                        id.replace('.', "-")
                    } else {
                        id
                    }
                })
            })
            .or_else(|| id.map(|id| format!("antigravity-model-{id}")))
            .unwrap_or_else(|| "unknown".into());
        let effort = display
            .and_then(effort)
            .or_else(|| raw.and_then(effort))
            .or_else(|| effort(&name))
            .map(str::to_owned);
        (name, effort)
    }
}
