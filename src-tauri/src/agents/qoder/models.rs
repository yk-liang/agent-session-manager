//! Qoder 模型名称：历史请求日志优先，官方模型列表补全，原始会话 ID 始终保留。

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::agents::{find_executable, open_jsonl, SessionMeta};

type Names = HashMap<String, String>;
const LOG_MARKER: &str = " INFO  debug.message [QoderInferRequest details] model_config=";
const MAX_LOG_LINE: u64 = 64 * 1024;
const REGISTRY_TTL: Duration = Duration::from_secs(300);

#[derive(Default)]
struct ModelNames {
    root: PathBuf,
    logs: HashMap<PathBuf, CachedLog>,
    registry: Names,
    /// 最近一次完整补全结果；列表读取只取快照，不等待日志扫描或 CLI。
    names: Names,
    registry_checked: Option<Instant>,
    /// 已尝试查找的标识；新模型可以立即补全，查无结果的旧标识不反复启动 CLI。
    queried: HashSet<String>,
}

struct CachedLog {
    stamp: (u64, SystemTime),
    names: Names,
}

#[derive(Deserialize)]
struct LoggedModel {
    key: String,
    display_name: String,
}

fn cache() -> &'static Mutex<ModelNames> {
    static CACHE: OnceLock<Mutex<ModelNames>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

pub(super) fn usage_names(root: &Path, ids: &[String]) -> Names {
    let Ok(cache)=cache().try_lock() else {return Names::new();};
    if cache.root!=root {return Names::new();}
    ids.iter().filter_map(|id|cache.names.get(id).map(|name|(id.clone(),name.clone()))).collect()
}

pub(super) fn apply_cached(root: &Path, sessions: &mut [SessionMeta]) {
    // 后台补全可能正在等待 CLI，列表与详情绝不能等待它持有的锁。
    let Ok(cache) = cache().try_lock() else {
        return;
    };
    if cache.root != root {
        return;
    }
    for session in sessions {
        session.model_name = session
            .model
            .as_ref()
            .and_then(|id| cache.names.get(id).cloned());
    }
}

pub(super) fn resolve(root: &Path, models: &[String]) -> Names {
    let wanted: HashSet<String> = models.iter().cloned().collect();
    if wanted.is_empty() {
        return Names::new();
    }
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    let names = cache.lookup(root, &wanted, || {
        let bin = find_executable("qodercli").or_else(|| {
            let entry = root.join("entry/qoder");
            entry.is_file().then_some(entry)
        });
        bin.map(|bin| registry_names(&bin, Duration::from_secs(5)))
            .unwrap_or_default()
    });
    cache.names = names.clone();
    names
        .into_iter()
        .filter(|(id, _)| wanted.contains(id))
        .collect()
}

impl ModelNames {
    fn lookup(
        &mut self,
        root: &Path,
        wanted: &HashSet<String>,
        registry: impl FnOnce() -> Names,
    ) -> Names {
        if self.root != root {
            *self = Self {
                root: root.to_path_buf(),
                ..Self::default()
            };
        }
        let mut present = HashSet::new();
        if let Ok(runs) = fs::read_dir(root.join("logs/runs")) {
            for run in runs.flatten() {
                let path = run.path().join("qodercli.log");
                let Ok(meta) = path.metadata() else { continue };
                if !meta.is_file() {
                    continue;
                }
                present.insert(path.clone());
                let stamp = (
                    meta.len(),
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                );
                if self.logs.get(&path).map(|cached| cached.stamp) != Some(stamp) {
                    if let Some(names) = log_names(&path) {
                        self.logs.insert(path, CachedLog { stamp, names });
                    }
                }
            }
        }
        self.logs.retain(|path, _| present.contains(path));
        let mut logs: Vec<_> = self.logs.iter().collect();
        logs.sort_by(|(a_path, a), (b_path, b)| {
            b.stamp.1.cmp(&a.stamp.1).then_with(|| b_path.cmp(a_path))
        });
        let mut names = Names::new();
        for (_, log) in logs {
            for (id, name) in &log.names {
                names.entry(id.clone()).or_insert_with(|| name.clone());
            }
        }
        let missing: HashSet<_> = wanted
            .iter()
            .filter(|id| !names.contains_key(*id))
            .cloned()
            .collect();
        let expired = self
            .registry_checked
            .map(|t| t.elapsed() >= REGISTRY_TTL)
            .unwrap_or(true);
        if !missing.is_empty() && (expired || missing.iter().any(|id| !self.queried.contains(id))) {
            self.registry.extend(registry());
            self.registry_checked = Some(Instant::now());
            if expired {
                self.queried.clear();
            }
            self.queried.extend(missing);
        }
        // 请求当时确认的名称优先于今天的模型列表，避免当前配置改写历史含义。
        for (id, name) in &self.registry {
            names.entry(id.clone()).or_insert_with(|| name.clone());
        }
        names
    }
}

fn log_names(path: &Path) -> Option<Names> {
    let mut reader = BufReader::new(open_jsonl(path).ok()?);
    let mut names = Names::new();
    let mut bytes = Vec::new();
    let mut skipping = false;
    loop {
        bytes.clear();
        if Read::by_ref(&mut reader)
            .take(MAX_LOG_LINE)
            .read_until(b'\n', &mut bytes)
            .ok()?
            == 0
        {
            break;
        }
        let complete = bytes.last() == Some(&b'\n');
        // 日志可能包含大段请求正文；按块跳过长行，只解析结构化的模型配置记录。
        if skipping || (!complete && bytes.len() as u64 == MAX_LOG_LINE) {
            skipping = !complete;
            continue;
        }
        let Ok(line) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Some((_, tail)) = line.split_once(LOG_MARKER) else {
            continue;
        };
        let Some(Ok(model)) = serde_json::Deserializer::from_str(tail)
            .into_iter::<LoggedModel>()
            .next()
        else {
            continue;
        };
        insert_name(&mut names, &model.key, &model.display_name);
    }
    Some(names)
}

fn insert_name(names: &mut Names, id: &str, name: &str) {
    let name = name.trim();
    if !id.is_empty()
        && !name.is_empty()
        && id.len() <= 512
        && name.len() <= 512
        && !id.chars().any(char::is_control)
        && !name.chars().any(char::is_control)
    {
        names.insert(id.to_string(), name.to_string());
    }
}

fn parse_registry(output: &str) -> Names {
    let mut names = Names::new();
    for row in output.lines().map(str::trim) {
        if row.is_empty() || row == "MODEL" {
            continue;
        }
        if let Some((name, id)) = row.strip_suffix(')').and_then(|row| row.rsplit_once(" (")) {
            insert_name(&mut names, id, name);
        } else if row
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/".contains(c))
        {
            insert_name(&mut names, row, row);
            insert_name(&mut names, &row.to_ascii_lowercase(), row);
        }
    }
    names
}

fn registry_names(bin: &Path, timeout: Duration) -> Names {
    let Ok(mut child) = Command::new(bin)
        .arg("--list-models")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Names::new();
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(25))
            }
            _ => {
                // 未登录、离线或 CLI 卡住只影响名称补全，不阻断历史会话读取。
                let _ = child.kill();
                let _ = child.wait();
                return Names::new();
            }
        }
    }
    let mut output = String::new();
    if child
        .stdout
        .take()
        .map(|out| out.take(256 * 1024).read_to_string(&mut output).is_ok())
        != Some(true)
    {
        return Names::new();
    }
    parse_registry(&output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_names_do_not_wait_for_an_in_progress_registry_lookup() {
        let guard = cache().lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            apply_cached(Path::new("/tmp/model-probe"), &mut []);
            tx.send(()).unwrap();
        });
        let completed = rx.recv_timeout(Duration::from_secs(1));
        drop(guard);
        assert!(completed.is_ok(), "目录读取不能等待模型补全锁");
    }

    fn write_log(root: &Path, run: &str, records: &str) -> PathBuf {
        let path = root.join("logs/runs").join(run).join("qodercli.log");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, records).unwrap();
        path
    }

    fn record(id: &str, name: &str) -> String {
        format!(
            "2026-10-15T16:01:14.710+08:00{LOG_MARKER}{} , custom_model=null\n",
            serde_json::json!({"key":id,"display_name":name,"api_key":"<REDACTED>"})
        )
    }

    #[test]
    fn resolves_multiple_historical_ids_and_custom_registry_names() {
        let root = tempfile::tempdir().unwrap();
        write_log(
            root.path(),
            "old",
            &record("mode-old", "Qwen3.8-Max-DogFooding"),
        );
        write_log(
            root.path(),
            "new",
            &record("mode-new", "Qwen3.8-Max-DogFooding"),
        );
        let wanted = [
            "mode-old",
            "mode-new",
            "internal-idealab/team",
            "auto",
            "mode-unknown",
        ]
        .map(String::from)
        .into_iter()
        .collect();
        let mut cache = ModelNames::default();
        let names = cache.lookup(root.path(), &wanted, || parse_registry(
            "MODEL\nAuto\nQwen3.8-Max-DogFooding (mode-new)\nTeam (Token Hub) (internal-idealab/team)\n"));
        assert_eq!(
            names.get("mode-old").map(String::as_str),
            Some("Qwen3.8-Max-DogFooding")
        );
        assert_eq!(names.get("mode-new"), names.get("mode-old"));
        assert_eq!(
            names.get("internal-idealab/team").map(String::as_str),
            Some("Team (Token Hub)")
        );
        assert_eq!(names.get("auto").map(String::as_str), Some("Auto"));
        assert!(!names.contains_key("mode-unknown"));
    }

    #[test]
    fn log_changes_refresh_names_without_requerying_missing_ids() {
        let root = tempfile::tempdir().unwrap();
        let path = write_log(root.path(), "run", &record("mode-old", "Recorded name"));
        let wanted = ["mode-old", "mode-unknown"]
            .map(String::from)
            .into_iter()
            .collect();
        let mut cache = ModelNames::default();
        let names = cache.lookup(root.path(), &wanted, || {
            parse_registry("New catalog name (mode-old)")
        });
        assert_eq!(names["mode-old"], "Recorded name");
        fs::write(&path, record("mode-old", "Updated recorded name")).unwrap();
        let names = cache.lookup(root.path(), &wanted, || {
            panic!("missing IDs should be cached")
        });
        assert_eq!(names["mode-old"], "Updated recorded name");
        fs::remove_file(path).unwrap();
        let names = cache.lookup(root.path(), &wanted, Names::new);
        assert_eq!(names["mode-old"], "New catalog name");
    }

    #[test]
    fn skips_malformed_or_oversized_log_lines_and_keeps_valid_names() {
        let root = tempfile::tempdir().unwrap();
        let content = format!(
            "{LOG_MARKER}{{broken}}\n{}\n{}{}",
            "x".repeat(MAX_LOG_LINE as usize + 10),
            record("mode-good", "Valid model"),
            record("mode-empty", " ")
        );
        let path = write_log(root.path(), "run", &content);
        let names = log_names(&path).unwrap();
        assert_eq!(names.len(), 1);
        assert_eq!(names["mode-good"], "Valid model");
    }

    #[test]
    fn cli_failures_and_timeouts_do_not_provide_misleading_names() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("qodercli");
        fs::write(
            &bin,
            "#!/bin/sh\nprintf 'Wrong model (mode-id)\\n'\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(registry_names(&bin, Duration::from_secs(1)).is_empty());
        fs::write(&bin, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
        let started = Instant::now();
        assert!(registry_names(&bin, Duration::from_millis(50)).is_empty());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn reads_official_cli_list_without_interactive_input() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("qodercli test");
        fs::write(&bin, "#!/bin/sh\n[ \"$1\" = '--list-models' ] || exit 1\nprintf 'MODEL\\nAuto\\nCustom model (mode-custom)\\n'\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        let names = registry_names(&bin, Duration::from_secs(1));
        assert_eq!(names["mode-custom"], "Custom model");
        assert_eq!(names["auto"], "Auto");
    }
}
