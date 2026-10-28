mod catalog_cache;
mod context;
#[cfg(test)]
mod context_test_support;
pub mod claude;
pub mod claude_family;
pub mod codex;
pub mod ids;
pub mod opencode;
pub mod pi;
pub mod profiles;
pub mod qoder;
pub mod safety;
pub mod scanning;
pub mod search;

pub use ids::escape_path_id;
pub use safety::{present, verify_within_root, TargetKind};
pub use scanning::{
    degraded_message, for_each_jsonl_line, open_jsonl, read_dir_warned, ParseOutcome,
};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 展开路径开头的 ~（用于环境变量覆盖值）
pub fn expand_tilde(p: &str) -> std::path::PathBuf {
    if p == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return dirs::home_dir().unwrap_or_default().join(rest);
    }
    std::path::PathBuf::from(p)
}

/// 毫秒时间戳 → ISO 字符串（UTC）
pub fn ms_to_iso(ms: i64) -> Option<String> {
    if ms <= 0 {
        return None;
    }
    let secs = ms / 1000;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant 的 civil-from-days 算法
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 {
        yoe + era * 400 + 1
    } else {
        yoe + era * 400
    };
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, mo, d, h, m, s
    ))
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub session_id: String,
    pub project_dir: String,
    pub cwd: Option<String>,
    pub title: Option<String>,
    /// 会话原始模型标识，保留用于排查，不以当前默认模型覆盖历史记录。
    pub model: Option<String>,
    /// 数据源确认的可读名称；缺失时由界面显示原始标识。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    pub version: Option<String>,
    pub git_branch: Option<String>,
    pub entrypoint: Option<String>,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
    pub user_count: u32,
    pub assistant_count: u32,
    /// 非 meta、有真实文本的用户消息数；为 0 表示空会话
    pub real_user_count: u32,
    /// 含正文文本的 assistant 消息数（排除纯工具调用/思考事件）
    pub assistant_text_count: u32,
    pub size_bytes: u64,
    pub has_transcript: bool,
    /// 首屏仅有文件头时尚无精确计数；后台摘要完成后清除。
    #[serde(default)]
    pub stats_pending: bool,
    /// 后端搜索定位及缓存校验用的源文件，SQLite 同时记录 WAL；不通过 IPC 暴露。
    #[serde(skip)]
    pub source_files: Vec<PathBuf>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub dir_name: String,
    pub display_path: String,
    pub session_count: usize,
    pub last_activity: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub meta: SessionMeta,
    pub events: Vec<serde_json::Value>,
    /// 可读取当前片段，但历史分页缺失时在打开详情后提示，不阻断已有内容。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// 一次扫描得到的完整目录：项目分组 + 全部会话 + 扫描告警
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub projects: Vec<Project>,
    pub sessions: Vec<SessionMeta>,
    /// 扫描中遇到的读取/解析失败（列表可能不完整时告知用户）
    pub warnings: Vec<String>,
    /// 桌面端明确登记为无项目的逻辑会话 ID；只供侧栏展示分组，不改变 project_dir。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub projectless_session_ids: Vec<String>,
    /// 按源目录与会话 ID 定位的展示归属；root=None 表示独立工作区，不改变真实 cwd。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub session_group_hints: Vec<SessionGroupHint>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionGroupHint {
    pub project_dir: String,
    pub session_id: String,
    pub root: Option<String>,
}

/// 会话删除的落地方式，决定 UI 措辞与可恢复性
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DeletePolicy {
    /// 移入系统回收站，可还原
    Trash,
    /// 永久删除（如委托官方 CLI 直接改库），不可还原
    Permanent,
}

/// 把 (会话, 项目展示路径) 列表聚合成 Catalog：
/// 按 project_dir 分组出项目统计，两组结果各按最近活跃时间排序。
pub fn assemble_catalog(items: Vec<(SessionMeta, String)>) -> Catalog {
    let mut groups: std::collections::BTreeMap<String, (String, Vec<SessionMeta>)> =
        std::collections::BTreeMap::new();
    let mut sessions = Vec::with_capacity(items.len());
    for (meta, display) in items {
        groups
            .entry(meta.project_dir.clone())
            .or_insert((display, Vec::new()))
            .1
            .push(meta.clone());
        sessions.push(meta);
    }
    let mut projects: Vec<Project> = groups
        .into_iter()
        .map(|(dir_name, (display_path, ms))| {
            let last_activity = ms.iter().filter_map(|s| s.last_ts.clone()).max();
            Project {
                session_count: ms.len(),
                last_activity,
                dir_name,
                display_path,
            }
        })
        .collect();
    projects.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    sessions.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    Catalog {
        projects,
        sessions,
        warnings: Vec::new(),
        projectless_session_ids: Vec::new(),
        session_group_hints: Vec::new(),
    }
}

/// 一个 agent 的单个配置文件描述符（用于配置快照）。
/// key 是稳定逻辑键（快照内标识，跨机器可移植）；path 是本机解析出的绝对路径。
pub struct ConfigFile {
    pub key: String,
    pub label: String,
    pub path: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: String,
    pub display_name: String,
    pub installed: bool,
    pub supports_fork: bool,
    /// 此数据源可在列表出现后补全/刷新模型名称。
    pub supports_model_names: bool,
    pub supports_fast_catalog: bool,
    /// 该 agent 的数据目录（用于「打开数据目录」）
    pub data_dir: Option<String>,
    /// 删除会话的落地方式（回收站 / 永久删除）
    pub delete_policy: DeletePolicy,
}

/// 相对 data_dir 的监听描述，不包含读取会话或识别 Agent 的逻辑。
#[derive(Clone, Default)]
pub struct AgentWatchPaths {
    pub transcript_dirs: Vec<PathBuf>,
    pub max_depth: Option<usize>,
    pub root_files: Vec<&'static str>,
    pub root_file_patterns: Vec<(&'static str, &'static str)>,
}

impl AgentWatchPaths {
    pub fn transcripts(directory: &str, max_depth: Option<usize>) -> Self {
        Self {
            transcript_dirs: vec![PathBuf::from(directory)],
            max_depth,
            ..Self::default()
        }
    }
}

/// 一个 CLI agent 的会话数据源。
/// 会话列表/详情统一为 agent 无关的模型；恢复命令等差异由各实现提供。
pub trait AgentSource: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    /// 本机是否存在该 agent 的数据目录
    fn is_installed(&self) -> bool;
    fn supports_fork(&self) -> bool {
        false
    }
    fn supports_model_names(&self) -> bool {
        false
    }
    /// 删除会话的落地方式；默认移入回收站
    fn delete_policy(&self) -> DeletePolicy {
        DeletePolicy::Trash
    }
    /// 数据目录（会话存储的根）
    fn data_dir(&self) -> Option<PathBuf>;
    /// usage 可包含归档、子代理及压缩日志；默认不猜测任何数据目录布局。
    fn watch_paths(&self, _usage: bool) -> AgentWatchPaths {
        AgentWatchPaths::default()
    }
    /// 一次扫描返回项目分组、全部会话与扫描告警（避免重复遍历历史数据）
    fn catalog(&self, include_empty: bool) -> Result<Catalog, String>;
    /// 默认数据源保持原行为；大日志数据源可先提供轻量目录。
    fn quick_catalog(&self, include_empty: bool) -> Result<Catalog, String> {
        self.catalog(include_empty)
    }
    fn supports_fast_catalog(&self) -> bool {
        false
    }
    /// 精确目录更新；支持增量检查的数据源可在读取各文件前响应取消。
    fn refine_catalog(
        &self,
        include_empty: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Catalog, String> {
        if cancelled() {
            return Err("目录更新已取消".into());
        }
        let catalog = self.catalog(include_empty)?;
        if cancelled() {
            return Err("目录更新已取消".into());
        }
        Ok(catalog)
    }
    /// None 表示不识别此令牌；识别后的失效/校验失败必须原样返回，不尝试其他数据源。
    fn read_tool_payload(&self, _token: &str) -> Option<Result<serde_json::Value, String>> {
        None
    }
    fn read_session_view(
        &self,
        project_dir: &str,
        session_id: &str,
    ) -> Result<SessionDetail, String> {
        self.read_session(project_dir, session_id)
    }
    /// 可选的外部模型名称补全。与目录扫描分开调用，不阻塞会话列表。
    fn resolve_model_names(&self, _ids: &[String]) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }
    fn read_session(&self, project_dir: &str, session_id: &str) -> Result<SessionDetail, String>;
    /// 使用目录扫描时确认的定位信息读取正文，避免逐会话重新遍历全部数据目录。
    fn search_events(&self, meta: &SessionMeta) -> Result<Vec<serde_json::Value>, String> {
        self.read_session(&meta.project_dir, &meta.session_id)
            .map(|detail| detail.events)
    }
    fn delete_session(&self, project_dir: &str, session_id: &str) -> Result<(), String>;
    /// 生成恢复会话的 shell 命令；None 表示该 agent 不支持恢复
    fn resume_command(&self, session_id: &str, fork: bool) -> Option<String>;
    /// 该 agent 的配置文件集合（用于配置快照）。
    /// 空 = 不支持配置快照；凭证/密钥文件一律不纳入。
    fn config_spec(&self) -> Vec<ConfigFile> {
        Vec::new()
    }
}

pub fn all_sources() -> Vec<Box<dyn AgentSource>> {
    vec![
        Box::new(qoder::QoderSource),
        Box::new(claude::ClaudeSource),
        Box::new(codex::CodexSource),
        Box::new(opencode::OpenCodeSource),
        Box::new(pi::PiSource),
    ]
}

pub fn find_source(id: &str) -> Result<Box<dyn AgentSource>, String> {
    all_sources()
        .into_iter()
        .find(|s| s.id() == id)
        .ok_or_else(|| format!("未知的 agent: {id}"))
}

/// 只接受 Agent 身份，由同一个 Adapter 解析数据根，避免前端传任意打开路径。
/// 隐藏目录、自定义目录和符号链接均受支持，不依赖会跳过点目录的 $HOME/** 通配规则。
pub fn data_directory(agent: &str) -> Result<PathBuf, String> {
    let path = find_source(agent)?
        .data_dir()
        .ok_or("该 Agent 没有配置数据目录")?;
    existing_directory(path)
}

fn existing_directory(path: PathBuf) -> Result<PathBuf, String> {
    let resolved = path.canonicalize().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => format!("数据目录不存在：{}", path.display()),
        std::io::ErrorKind::PermissionDenied => {
            format!("系统拒绝访问数据目录，请检查该目录的访问权限：{}", path.display())
        }
        _ => format!("无法访问数据目录 {}：{error}", path.display()),
    })?;
    let metadata = resolved
        .metadata()
        .map_err(|error| format!("无法读取数据目录信息：{error}"))?;
    if !metadata.is_dir() {
        return Err(format!("数据目录路径不是文件夹：{}", path.display()));
    }
    Ok(resolved)
}

/// IPC 仅传递不透明令牌；实际定位及完整性校验由登记该令牌的 Adapter 完成。
pub fn read_tool_payload(token: &str) -> Result<serde_json::Value, String> {
    all_sources()
        .into_iter()
        .find_map(|source| source.read_tool_payload(token))
        .unwrap_or_else(|| Err("工具内容引用无效或已过期，请重新打开会话".into()))
}

pub fn list_agents() -> Vec<AgentInfo> {
    all_sources()
        .iter()
        .map(|s| AgentInfo {
            id: s.id().to_string(),
            display_name: s.display_name().to_string(),
            installed: s.is_installed(),
            supports_fork: s.supports_fork(),
            supports_model_names: s.supports_model_names(),
            supports_fast_catalog: s.supports_fast_catalog(),
            data_dir: s.data_dir().map(|p| p.to_string_lossy().to_string()),
            delete_policy: s.delete_policy(),
        })
        .collect()
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 解析可执行文件的绝对路径。
/// macOS 下从 Finder/Dock 启动的 GUI 应用只继承精简 PATH
///（/usr/bin:/bin:/usr/sbin:/sbin），拿不到用户 shell 里配置的
/// Homebrew/npm/bun 等安装目录，直接 `Command::new("opencode")` 会
/// ENOENT（No such file or directory）。这里在继承的 PATH 之外再
/// 补充常见安装目录，逐个查实际存在且带执行位的文件，返回首个
/// 命中；找不到返回 None（调用方须显式报错，绝不静默降级）。
pub fn find_executable(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let mut dirs: Vec<PathBuf> = Vec::new();
    // 1) 继承的 PATH（终端/dev 启动时已足够；空路径段跳过）
    if let Ok(path) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&path).filter(|p| !p.as_os_str().is_empty()));
    }
    // 2) GUI 精简 PATH 下的常见安装目录兑底
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    if let Some(home) = dirs::home_dir() {
        for sub in [
            ".local/bin",
            ".opencode/bin",
            ".bun/bin",
            ".npm-global/bin",
            ".volta/bin",
        ] {
            dirs.push(home.join(sub));
        }
    }
    for dir in dirs {
        let candidate = dir.join(name);
        // metadata 跟随符号链接（Homebrew 的 bin 多为软链接），
        // 只接受带执行位的普通文件
        if let Ok(meta) = candidate.metadata() {
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                return Some(candidate);
            }
        }
    }
    None
}

/// 在新终端窗口中执行命令（仅 macOS，通过 osascript 驱动 Terminal）
#[cfg(target_os = "macos")]
pub fn run_in_terminal(cmd: &str) -> Result<(), String> {
    let script = format!(
        "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
        cmd.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("调用 osascript 失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "打开终端失败: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn run_in_terminal(_cmd: &str) -> Result<(), String> {
    Err("终端恢复目前仅支持 macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_directory_accepts_hidden_custom_and_linked_directories() {
        let fixture = tempfile::tempdir().unwrap();
        for relative in [
            ".codex", ".local/share/opencode", ".claude", ".qoder", ".pi/agent",
            "custom/数据 ' [目录]",
        ] {
            let path = fixture.path().join(relative);
            std::fs::create_dir_all(&path).unwrap();
            assert_eq!(
                existing_directory(path.clone()).unwrap(),
                path.canonicalize().unwrap()
            );
        }
        let link = fixture.path().join("linked-root");
        let target = fixture.path().join("custom/数据 ' [目录]");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            existing_directory(link).unwrap(),
            target.canonicalize().unwrap()
        );
    }

    #[test]
    fn data_directory_rejects_missing_paths_files_and_unregistered_agents() {
        let fixture = tempfile::tempdir().unwrap();
        let missing = fixture.path().join("missing");
        assert!(existing_directory(missing.clone()).unwrap_err().contains("不存在"));
        assert!(
            !missing.exists(),
            "opening must not create an empty data directory"
        );
        let file = fixture.path().join("config.json");
        std::fs::write(&file, "{}").unwrap();
        assert!(existing_directory(file.clone()).unwrap_err().contains("不是文件夹"));
        for value in ["unknown", "/tmp", "../codex", file.to_str().unwrap()] {
            assert!(data_directory(value).unwrap_err().contains("未知的 agent"));
        }
    }

    struct FixtureSource(std::sync::atomic::AtomicUsize);
    impl AgentSource for FixtureSource {
        fn id(&self) -> &'static str {
            "fixture"
        }
        fn display_name(&self) -> &'static str {
            "Fixture"
        }
        fn is_installed(&self) -> bool {
            true
        }
        fn data_dir(&self) -> Option<PathBuf> {
            None
        }
        fn catalog(&self, _: bool) -> Result<Catalog, String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(assemble_catalog(vec![]))
        }
        fn read_session(&self, _: &str, _: &str) -> Result<SessionDetail, String> {
            unreachable!()
        }
        fn delete_session(&self, _: &str, _: &str) -> Result<(), String> {
            unreachable!()
        }
        fn resume_command(&self, _: &str, _: bool) -> Option<String> {
            None
        }
    }

    #[test]
    fn refinement_defaults_honor_cancellation_without_agent_specific_dispatch() {
        use std::sync::atomic::Ordering;
        let source = FixtureSource(Default::default());
        assert!(source.refine_catalog(false, &|| true).is_err());
        assert_eq!(source.0.load(Ordering::SeqCst), 0);
        assert!(source
            .refine_catalog(false, &|| source.0.load(Ordering::SeqCst) > 0)
            .is_err());
        assert_eq!(source.0.load(Ordering::SeqCst), 1);
        assert!(source.refine_catalog(false, &|| false).is_ok());
        assert!(source.read_tool_payload("anything").is_none());
        assert!(source.watch_paths(false).transcript_dirs.is_empty());
        assert!(codex::CodexSource.refine_catalog(false, &|| true).is_err());
    }

    #[test]
    fn tool_payload_router_rejects_unregistered_tokens_and_paths() {
        let file = tempfile::NamedTempFile::new().unwrap();
        for token in [
            file.path().to_str().unwrap(),
            "opencode:/tmp/secret:input",
            "opencode:unknown:result",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert!(read_tool_payload(token).is_err());
        }
    }

    #[test]
    fn find_executable_locates_known_bin_and_rejects_bogus() {
        // /bin/sh 在精简 PATH 中也能找到，且应是绝对路径
        let sh = find_executable("sh").expect("sh 应可解析");
        assert!(sh.is_absolute());
        assert!(sh.ends_with("sh"));
        // 不存在的名字返回 None，不能误报
        assert!(find_executable("asm-definitely-no-such-bin-xyz").is_none());
    }

    #[test]
    fn find_executable_skips_non_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        // 在临时目录放一个同名但不带执行位的普通文件，
        // 把该目录放到 PATH 首位：不应被当作可执行命中
        let dir = tempfile::tempdir().unwrap();
        let name = "asm-nonexec-probe";
        let f = dir.path().join(name);
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let prev = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{}", dir.path().display(), prev));
        let got = find_executable(name);
        std::env::set_var("PATH", prev);
        assert!(got.is_none(), "不带执行位的文件不应命中");
    }
}
