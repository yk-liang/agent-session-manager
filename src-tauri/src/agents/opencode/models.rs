//! 模型身份来自历史 assistant 消息；本地目录和配置只补全名称，不能改变历史模型。
use super::*;
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Clone)]
pub(super) struct Identity {
    pub id: String,
    pub label: String,
}
fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
pub(super) fn from_message(message: &Value) -> Option<Identity> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let nested = message.get("model");
    let model = text(message.get("modelID"))
        .or_else(|| text(nested.and_then(|v| v.get("modelID").or_else(|| v.get("id")))))
        .or_else(|| text(nested))?;
    let provider =
        text(message.get("providerID")).or_else(|| text(nested.and_then(|v| v.get("providerID"))));
    Some(Identity {
        id: provider
            .map(|p| format!("{p}/{model}"))
            .unwrap_or_else(|| model.into()),
        label: model.into(),
    })
}
impl Identity {
    pub fn apply(&self, meta: &mut SessionMeta) {
        meta.model = Some(self.id.clone());
        meta.model_name = Some(self.label.clone());
    }
}

/// 一次读取轻量消息元数据，不连接 part 表，也不逐会话扫描工具输出。
pub(super) fn latest(
    conn: &Connection,
    session: Option<&str>,
    warnings: &mut Vec<String>,
) -> HashMap<String, Identity> {
    let filter = if session.is_some() {
        "session_id=?1 AND "
    } else {
        ""
    };
    let order = storage::order_column(conn);
    let query = format!(
        "SELECT session_id,json_object('role','assistant','modelID',json_extract(data,'$.modelID'),
        'providerID',json_extract(data,'$.providerID'),'model',json_extract(data,'$.model'))
        FROM message WHERE {filter}json_valid(data) AND json_extract(data,'$.role')='assistant'
        ORDER BY {order},id"
    );
    let read = || -> Result<HashMap<String, Identity>, rusqlite::Error> {
        let mut stmt = conn.prepare(&query)?;
        let decode =
            |row: &rusqlite::Row<'_>| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?));
        let rows = if let Some(id) = session {
            stmt.query_map([id], decode)?
        } else {
            stmt.query_map([], decode)?
        };
        let mut models = HashMap::new();
        for row in rows {
            let (id, data) = row?;
            if let Ok(value) = serde_json::from_str::<Value>(&data) {
                if let Some(model) = from_message(&value) {
                    models.insert(id, model);
                }
            }
        }
        Ok(models)
    };
    read().unwrap_or_else(|_| {
        warnings.push("OpenCode 历史模型读取失败，当前保留其他会话信息".into());
        HashMap::new()
    })
}

pub(super) type Names = HashMap<String, String>;
#[derive(Clone, PartialEq, Eq)]
struct Revision {
    path: PathBuf,
    state: Option<(u64, Option<SystemTime>, u64, i64, i64)>,
}
fn revisions(paths: &[PathBuf]) -> Vec<Revision> {
    paths
        .iter()
        .map(|p| Revision {
            path: p.clone(),
            state: p.metadata().ok().map(|m| {
                (
                    m.len(),
                    m.modified().ok(),
                    m.ino(),
                    m.ctime(),
                    m.ctime_nsec(),
                )
            }),
        })
        .collect()
}
#[derive(Default)]
struct NameCache(Mutex<Option<(Vec<Revision>, Arc<Names>)>>);
impl NameCache {
    fn load(&self, paths: &[PathBuf]) -> Arc<Names> {
        let before = revisions(paths);
        if let Some((revision, names)) = &*self.0.lock().unwrap_or_else(|e| e.into_inner()) {
            if *revision == before {
                return names.clone();
            }
        }
        let mut names = Names::new();
        for (index, path) in paths.iter().enumerate() {
            let Ok(mut file) = super::super::open_jsonl(path) else {
                continue;
            };
            if file
                .metadata()
                .map(|m| m.len() > 32 * 1024 * 1024)
                .unwrap_or(true)
            {
                continue;
            }
            let mut data = String::new();
            if file.read_to_string(&mut data).is_err() {
                continue;
            }
            let value = if index == 0 {
                serde_json::from_str::<Value>(&data).ok()
            } else {
                jsonc_parser::parse_to_serde_value::<Value>(&data, &Default::default()).ok()
            };
            let Some(value) = value else { continue };
            let providers = if index == 0 {
                Some(&value)
            } else {
                // v2 uses plural `providers`; prefer it when both versions are present.
                value.get("providers").or_else(|| value.get("provider"))
            };
            let Some(providers) = providers.and_then(Value::as_object) else {
                continue;
            };
            for (provider, info) in providers {
                let Some(models) = info.get("models").and_then(Value::as_object) else {
                    continue;
                };
                for (id, model) in models {
                    if let Some(name) = text(model.get("name")) {
                        names.insert(format!("{provider}/{id}"), name.into());
                    }
                }
            }
        }
        let names = Arc::new(names);
        if before == revisions(paths) {
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((before, names.clone()));
        }
        names
    }
}

fn name_paths() -> Vec<PathBuf> {
    // OpenCode 目录缓存为基础，用户配置中的同 ID 自定义名称优先。
    let cache = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| super::super::expand_tilde(&s))
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"));
    vec![
        cache.join("opencode/models.json"),
        config_home().join("opencode.json"),
        config_home().join("opencode.jsonc"),
    ]
}
pub(super) fn names() -> Arc<Names> {
    static CACHE: OnceLock<NameCache> = OnceLock::new();
    CACHE.get_or_init(NameCache::default).load(&name_paths())
}
pub(super) fn decorate_meta(meta: &mut SessionMeta, names: &Names) {
    if let Some(name) = meta.model.as_ref().and_then(|id| names.get(id)) {
        meta.model_name = Some(name.clone());
    }
}
pub(super) fn decorate_detail(detail: &mut SessionDetail, names: &Names) {
    decorate_meta(&mut detail.meta, names);
    for event in &mut detail.events {
        if let Some(name) = event
            .pointer("/message/model")
            .and_then(Value::as_str)
            .and_then(|id| names.get(id))
        {
            event["message"]["modelName"] = json!(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_names_are_provider_scoped_and_follow_config_changes_without_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let registry = dir.path().join("models.json");
        let json = dir.path().join("opencode.json");
        let jsonc = dir.path().join("opencode.jsonc");
        std::fs::write(&registry,r#"{"a":{"models":{"same":{"name":"Registry A"}}},"b":{"models":{"same":{"name":"Registry B"}}}}"#).unwrap();
        std::fs::write(&json,r#"{"model":"b/current-default","provider":{"a":{"options":{"apiKey":"SHOULD_NOT_BE_MODEL"},"models":{"same":{"name":"Custom A"}}}}}"#).unwrap();
        std::fs::write(&jsonc,"{ // comment\n \"provider\": {\"a\":{\"models\":{\"same\":{\"name\":\"JSONC A\"},},},},}").unwrap();
        let paths = vec![registry, json.clone(), jsonc.clone()];
        let cache = NameCache::default();
        let names = cache.load(&paths);
        assert_eq!(names.get("a/same").map(String::as_str), Some("JSONC A"));
        assert_eq!(names.get("b/same").map(String::as_str), Some("Registry B"));
        assert!(!names.values().any(|n| n.contains("SHOULD_NOT")));
        std::fs::remove_file(jsonc).unwrap();
        assert_eq!(
            cache.load(&paths).get("a/same").map(String::as_str),
            Some("Custom A")
        );
        std::fs::write(&json, r#"{"providers":{"a":{"settings":{"apiKey":"SHOULD_NOT_BE_MODEL"},"models":{"same":{"name":"V2 A"}}}},"provider":{"a":{"models":{"same":{"name":"Old A"}}}}}"#).unwrap();
        assert_eq!(
            cache.load(&paths).get("a/same").map(String::as_str),
            Some("V2 A")
        );
        std::fs::write(json, "broken JSON").unwrap();
        assert_eq!(
            cache.load(&paths).get("a/same").map(String::as_str),
            Some("Registry A")
        );
    }
    #[test]
    fn missing_or_nested_model_fields_do_not_use_user_requested_models() {
        assert!(from_message(&json!({"role":"user","model":{"modelID":"not-used"}})).is_none());
        assert_eq!(
            from_message(
                &json!({"role":"assistant","model":{"providerID":"legacy","modelID":"id"}})
            )
            .unwrap()
            .id,
            "legacy/id"
        );
        assert_eq!(
            from_message(&json!({"role":"assistant","modelID":"local"}))
                .unwrap()
                .id,
            "local"
        );
        assert!(from_message(&json!({"role":"assistant","modelID":42})).is_none());
    }
}
