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
        let raw = context
            .and_then(|m| m.name.as_deref())
            .or_else(|| observed.and_then(|m| m.name.as_deref()));
        let name = id
            .and_then(known_name)
            .map(str::to_owned)
            .or_else(|| {
                raw.and_then(|raw| {
                    let lower = raw.to_ascii_lowercase();
                    if let Some(number) = lower.strip_prefix("model_placeholder_m") {
                        return known_name(number.parse::<u64>().ok()?.checked_add(1000)?)
                            .map(str::to_owned);
                    }
                    Some(
                        match lower.as_str() {
                            "gemini-pro-default" | "gemini-pro-agent" => "gemini-3.1-pro",
                            "gemini-3-flash-a"
                            | "gemini-3-flash-b"
                            | "gemini-3-flash-agent"
                            | "gemini-3-flash-agent-a"
                            | "gemini-3-flash-agent-b" => "gemini-3.5-flash-high",
                            "gemini-3-flash-c" | "gemini-3-flash" => "gemini-3-flash-preview",
                            _ => raw,
                        }
                        .to_owned(),
                    )
                })
            })
            .or_else(|| {
                display.map(|name| {
                    name.split(" (")
                        .next()
                        .unwrap_or(name)
                        .to_lowercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join("-")
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
