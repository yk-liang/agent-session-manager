//! 多 Agent 配置方案快照：由各数据源声明受管理的配置文件，负责
//! 保存、列表、应用（持久事务与安全恢复）与删除。
//!
//! 安全要点：
//! - 配置可能含凭据：存储目录强制 0700，写入文件一律 0600
//! - 方案列表只返回摘要；文件内容和差异按需返回，不写入日志
//! - 配置库事务（list/save/delete/apply）由 flock 排他锁串行化：
//!   单实例插件在 macOS 有探测竞态，不能替代跨进程互斥
//! - 应用前把所有原文件克隆进各自专属的暂存事务目录（保留权限/ACL），
//!   新内容原子替换；失败时暂存回改名恢复，绝不把"读不到"当成"不存在"

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::ConfigFile;

mod transaction;
pub mod workspace;

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 快照中的单个配置文件：key 是稳定逻辑键，content 是原始文本内容
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshot {
    pub key: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    pub agent: String,
    pub created_at: u64,
    pub files: Vec<ConfigSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_keys: Option<Vec<String>>,
}

/// 前端可见的单文件在场标记（不含内容）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePresence {
    pub key: String,
    pub label: String,
    pub present: bool,
}

/// 前端可见的配置方案摘要：不含任何配置内容（可能包括凭据）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub name: String,
    pub agent: String,
    pub created_at: u64,
    pub files: Vec<FilePresence>,
}

/// 依据 agent 的 config_spec 生成摘要：逐文件标出快照中是否留有内容
fn summarize(p: &Profile, spec: &[ConfigFile]) -> ProfileSummary {
    let files = spec
        .iter()
        .map(|c| FilePresence {
            key: c.key.clone(),
            label: c.label.clone(),
            present: p.files.iter().any(|f| f.key == c.key),
        })
        .collect();
    ProfileSummary {
        name: p.name.clone(),
        agent: p.agent.clone(),
        created_at: p.created_at,
        files,
    }
}

#[derive(Serialize, Default)]
struct ProfileStore {
    profiles: Vec<Profile>,
}

/// 反序列化中转：兼容旧格式（Qoder 专属的 settings/mcp/modelsDefault 字段）
#[derive(Deserialize, Default)]
struct RawStore {
    #[serde(default)]
    profiles: Vec<RawProfile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProfile {
    name: String,
    #[serde(default)]
    agent: Option<String>,
    created_at: u64,
    #[serde(default)]
    files: Option<Vec<ConfigSnapshot>>,
    #[serde(default)]
    captured_keys: Option<Vec<String>>,
    // 旧格式字段（解析后的 JSON），仅在缺少 files 时用于一次性迁移
    #[serde(default)]
    settings: Option<Value>,
    #[serde(default)]
    mcp: Option<Value>,
    #[serde(default)]
    models_default: Option<Value>,
}

impl RawProfile {
    /// 归一到新格式：已是 files 直接用；旧格式迁移为 agent=qoder + 原始文本 files
    fn into_profile(self) -> Profile {
        let agent = self.agent.unwrap_or_else(|| "qoder".to_string());
        if let Some(files) = self.files {
            return Profile {
                name: self.name,
                agent,
                created_at: self.created_at,
                files,
                captured_keys: self.captured_keys,
            };
        }
        let mut files = Vec::new();
        for (key, v) in [
            ("settings.json", self.settings),
            ("mcp.json", self.mcp),
            (".models/default", self.models_default),
        ] {
            if let Some(v) = v {
                // 迁移一次性把解析后的 JSON 转为美化文本，之后按原始字节管理
                let content = serde_json::to_string_pretty(&v).unwrap_or_default();
                files.push(ConfigSnapshot {
                    key: key.to_string(),
                    content,
                });
            }
        }
        // 只有旧 Qoder 字段格式能证明完整采集范围。其他 agent 缺少
        // files 时不能凭此继承 settings.json 的删除权限。
        let captured_keys = (agent == "qoder").then(|| {
            vec![
                "settings.json".to_string(),
                "mcp.json".to_string(),
                ".models/default".to_string(),
            ]
        });
        Profile {
            name: self.name,
            agent,
            created_at: self.created_at,
            files,
            captured_keys,
        }
    }
}

/// 当前受管理文件；排除独立凭证文件，但正文仍可能内嵌密钥，不应写入日志。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentFile {
    pub key: String,
    pub label: String,
    pub path: String,
    pub present: bool,
    pub content: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentConfig {
    pub agent: String,
    pub files: Vec<CurrentFile>,
}

fn store_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "无法确定 home 目录".to_string())?;
    let new_dir = home.join(".agent-session-manager");
    // 从旧名 ~/.qoder-session-manager 自动迁移；探测严格化
    // （只有 NotFound 才算缺失，探测错误显性化），
    // 迁移失败必须报错，否则会打开空的新库，历史配置看似"消失"
    let old_dir = home.join(".qoder-session-manager");
    if !super::present(&new_dir)? && super::present(&old_dir)? {
        fs::rename(&old_dir, &new_dir).map_err(|e| {
            format!(
                "迁移配置目录 {:?} → {:?} 失败: {e}；请人工处理，历史配置仍在旧目录中",
                old_dir, new_dir
            )
        })?;
    }
    Ok(new_dir)
}

/// 配置库可能含凭据：目录强制 0700。
/// 创建时就用 DirBuilder 指定 0700（umask 只能减权限不能加，
/// 结果至多 0700，宽松 umask 下也私有）；随后的 chmod 是
/// 为收紧旧版本遗留的宽权限目录。符号链接目录会让
/// set_permissions 跟随 chmod 目标，必须拒绝
fn ensure_store_dir() -> Result<PathBuf, String> {
    let dir = store_dir()?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(&dir)
        .map_err(|e| format!("创建配置目录 {:?} 失败: {e}", dir))?;
    let meta = dir
        .symlink_metadata()
        .map_err(|e| format!("无法读取配置目录 {:?}: {e}", dir))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(format!(
            "配置目录 {:?} 不是真实目录（可能是符号链接），请先人工处理",
            dir
        ));
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("设置配置目录权限 {:?} 失败: {e}", dir))?;
    fs::File::open(&dir)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("持久化配置目录 {:?} 失败: {e}", dir))?;
    if let Some(parent) = dir.parent() {
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| format!("持久化配置父目录 {:?} 失败: {e}", parent))?;
    }
    Ok(dir)
}

fn store_path() -> Result<PathBuf, String> {
    Ok(store_dir()?.join("profiles.json"))
}

/// 配置库事务排他锁的持有凭证：drop 即关闭句柄、释放 flock
#[derive(Debug)]
struct StoreLockGuard {
    _file: fs::File,
}

/// 在给定目录上获取配置库事务排他锁（可注入，测试用 tempdir）。
/// tauri-plugin-single-instance 在 macOS 上先探测 socket 再异步建监听，
/// 双进程同时启动可能都通过探测（监听建立失败者仅记日志继续启动），
/// 无法保证互斥；因此这里用 flock 同步阻塞获取排他锁，
/// 对协作进程有效，堵住双实例交错事务导致的更新丢失。
/// 锁文件只用于加锁、从不写内容；拒绝符号链接冒充锁文件。
fn lock_store_in(dir: &Path) -> Result<StoreLockGuard, String> {
    let lock_path = dir.join(".store.lock");
    let file = match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
    {
        Ok(f) => f,
        // 已存在：以 no-follow + nonblock 方式打开。O_NOFOLLOW 拒绝
        // 符号链接冒充；O_NONBLOCK 防止无读端的 FIFO write-open 在
        // flock 之前永久阻塞（无读端时直接报 ENXIO）
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut opts = fs::OpenOptions::new();
            opts.write(true);
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            let f = opts
                .open(&lock_path)
                .map_err(|e| format!("打开配置库锁文件 {:?} 失败: {e}", lock_path))?;
            // fd 元数据即实际打开对象的身份：必须是普通文件，
            // FIFO/目录等特殊文件一律拒绝，绝不带着它进入 flock
            //（O_NONBLOCK 不影响后续 flock 的阻塞语义）
            let meta = f
                .metadata()
                .map_err(|e| format!("读取配置库锁文件 {:?} 元数据失败: {e}", lock_path))?;
            if !meta.is_file() {
                return Err(format!(
                    "配置库锁文件 {:?} 不是普通文件（FIFO/目录等），请先人工处理",
                    lock_path
                ));
            }
            f
        }
        Err(e) => return Err(format!("创建配置库锁文件 {:?} 失败: {e}", lock_path)),
    };
    // 同步阻塞获取排他锁：覆盖 list/save/delete/apply 整个事务
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(format!(
            "获取配置库排他锁 {:?} 失败: {}",
            lock_path,
            std::io::Error::last_os_error()
        ));
    }
    Ok(StoreLockGuard { _file: file })
}

/// 配置库事务排他锁：跨进程（flock）+ 跨线程（每个句柄独立加锁）。
/// 原子 rename 只能防文件损坏，防不住两个保存/删除请求的读-改-写
/// 互相覆盖（配置丢失、删除后复活）；apply 也持锁，
/// 避免并发应用在同一批配置文件上交错暂存。
fn store_lock() -> Result<StoreLockGuard, String> {
    let dir = ensure_store_dir()?;
    let guard = lock_store_in(&dir)?;
    let spec: Vec<_> = super::all_sources()
        .into_iter()
        .flat_map(|s| s.config_spec())
        .collect();
    transaction::recover_in(&dir, &spec)?;
    Ok(guard)
}

/// 可在启动时调用；错误只作为配置恢复告警处理，不应中止整个应用启动。
/// 后续配置访问仍会在同一 flock 内重试，并返回可处理的冲突错误。
pub fn recover_pending_config_transactions() -> Result<(), String> {
    let _guard = store_lock()?;
    Ok(())
}

/// 读取时即收紧遗留配置库权限：目录 0700、文件 0600。
/// 从旧目录迁移来的库可能一直是 0755/0644 且含凭据，不能等下次保存。
/// 严格类型校验：只对真实目录与普通文件 chmod；符号链接一律拒绝
/// （set_permissions 会跟随链接 chmod 目标），目录/FIFO 等特殊文件
/// 同样拒绝（FIFO 会让后续读取永久阻塞）。
fn tighten_store_permissions(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        match dir.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("无法读取配置目录 {:?}: {e}", dir)),
            Ok(m) => {
                if m.file_type().is_symlink() || !m.is_dir() {
                    return Err(format!(
                        "配置目录 {:?} 不是真实目录（可能是符号链接），请先人工处理",
                        dir
                    ));
                }
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                    .map_err(|e| format!("收紧配置目录权限 {:?} 失败: {e}", dir))?;
            }
        }
    }
    match path.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("无法读取配置库 {:?}: {e}", path)),
        Ok(m) => {
            if m.file_type().is_symlink() {
                return Err(format!(
                    "配置库 {:?} 是符号链接，拒绝跟随收紧权限，请先人工处理",
                    path
                ));
            }
            if !m.is_file() {
                return Err(format!(
                    "配置库 {:?} 不是普通文件（目录/FIFO 等），请先人工处理",
                    path
                ));
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("收紧配置库权限 {:?} 失败: {e}", path))?;
        }
    }
    Ok(())
}

/// 读取配置库内容：O_NOFOLLOW 打开防止跟随符号链接，
/// O_NONBLOCK 保证即使被换成 FIFO 也不会阻塞；
/// 打开后再用 fd 元数据复核普通文件，消除检查/使用竞态。
/// 不存在 = Ok(None)；其余失败一律 Err。
fn read_store_file(path: &Path) -> Result<Option<String>, String> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("无法读取配置库 {:?}: {e}", path)),
    };
    // fd 元数据即实际打开对象的身份，无竞态窗口
    let meta = file
        .metadata()
        .map_err(|e| format!("无法读取配置库 {:?}: {e}", path))?;
    if !meta.is_file() {
        return Err(format!("配置库 {:?} 不是普通文件，拒绝读取", path));
    }
    let mut raw = String::new();
    file.read_to_string(&mut raw)
        .map_err(|e| format!("读取配置库 {:?} 失败: {e}", path))?;
    Ok(Some(raw))
}

fn load_store() -> Result<ProfileStore, String> {
    let path = store_path()?;
    tighten_store_permissions(&path)?;
    let Some(raw) = read_store_file(&path)? else {
        // 文件不存在 = 还没有任何配置，属正常情况
        return Ok(ProfileStore::default());
    };
    let raw: RawStore = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "配置库 {:?} 已损坏（{e}），请先人工处理，避免覆盖现有数据",
            path
        )
    })?;
    // 旧格式 profile 在此归一化迁移，历史 Qoder 快照不丢失
    Ok(ProfileStore {
        profiles: raw
            .profiles
            .into_iter()
            .map(RawProfile::into_profile)
            .collect(),
    })
}

fn save_store(store: &ProfileStore) -> Result<(), String> {
    let dir = ensure_store_dir()?;
    let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    atomic_write(&dir.join("profiles.json"), json.as_bytes())
}

/// 原子写：唯一临时文件（0600）写满后 rename。
/// rename 后目标继承临时文件的 0600，不会把已有的紧权限放宽。
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut f = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("写入临时文件 {:?} 失败: {e}", tmp))?;
    if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("写入临时文件 {:?} 失败: {e}", tmp));
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("替换 {:?} 失败: {e}", path)
    })?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| format!("持久化目录 {:?} 失败: {e}", parent))?;
    }
    Ok(())
}

/// 读取配置文件原始字节：不存在 = Ok(None)；读取失败 = Err。
/// 不做格式解析（TOML/JSONC/JSON 通吃）；O_NONBLOCK + fd 普通文件复核
/// 挡住 FIFO/目录等特殊文件（FIFO 会永久阻塞）。这里不加 O_NOFOLLOW：
/// agent 自己的配置可能是 dotfiles 管理的符号链接，跟随到普通文件读取是正常预期
///（写入事务仍会显式拒绝符号链接，避免悄悄替换）。
/// 有字节即 present、忠实快照：旧“损坏 JSON 不得当不存在”的隐患自然消解。
fn read_config_raw(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    opts.custom_flags(libc::O_NONBLOCK);
    let mut file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("无法读取配置文件 {:?}: {e}", path)),
    };
    let meta = file
        .metadata()
        .map_err(|e| format!("无法读取配置文件 {:?}: {e}", path))?;
    if !meta.is_file() {
        return Err(format!("配置文件 {:?} 不是普通文件，拒绝读取", path));
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("读取配置文件 {:?} 失败: {e}", path))?;
    Ok(Some(buf))
}

/// 把原始字节按 UTF-8 解读为文本；非 UTF-8 显式报错，绝不静默降级
fn bytes_to_text(path: &Path, bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| format!("配置文件 {:?} 不是 UTF-8 文本", path))
}

pub fn get_current_config(agent: &str) -> Result<CurrentConfig, String> {
    let _guard = store_lock()?;
    let spec = super::find_source(agent)?.config_spec();
    let mut files = Vec::new();
    for c in spec {
        let content = match read_config_raw(&c.path)? {
            Some(bytes) => Some(bytes_to_text(&c.path, bytes)?),
            None => None,
        };
        files.push(CurrentFile {
            key: c.key,
            label: c.label,
            path: c.path.to_string_lossy().to_string(),
            present: content.is_some(),
            content,
        });
    }
    Ok(CurrentConfig {
        agent: agent.to_string(),
        files,
    })
}

pub fn list_profiles(agent: &str) -> Result<Vec<ProfileSummary>, String> {
    let _guard = store_lock()?;
    let spec = super::find_source(agent)?.config_spec();
    Ok(load_store()?
        .profiles
        .iter()
        .filter(|p| p.agent == agent)
        .map(|p| summarize(p, &spec))
        .collect())
}

pub fn save_profile(
    agent: &str,
    name: String,
    expected_revision: &str,
) -> Result<ProfileSummary, String> {
    let _guard = store_lock()?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("名称不能为空".into());
    }
    let source = super::find_source(agent)?;
    let spec = source.config_spec();
    if spec.is_empty() {
        return Err(format!("{} 不支持配置快照", source.display_name()));
    }
    workspace::check_revision(&spec, expected_revision)?;
    let mut store = load_store()?;
    // 名称唯一性按 (agent, name)：不同 agent 可重名
    if store
        .profiles
        .iter()
        .any(|p| p.agent == agent && p.name == name)
    {
        return Err(format!("配置 “{name}” 已存在"));
    }
    let mut snapshot = Vec::new();
    for c in &spec {
        if let Some(bytes) = read_config_raw(&c.path)? {
            let content = bytes_to_text(&c.path, bytes)?;
            workspace::validate_document(&c.key, &content)?;
            snapshot.push(ConfigSnapshot {
                key: c.key.clone(),
                content,
            });
        }
    }
    workspace::check_snapshot_revision(&spec, &snapshot, expected_revision)?;
    let profile = Profile {
        name,
        agent: agent.to_string(),
        created_at: now_millis(),
        files: snapshot,
        captured_keys: Some(spec.iter().map(|config| config.key.clone()).collect()),
    };
    let summary = summarize(&profile, &spec);
    store.profiles.push(profile);
    save_store(&store)?;
    Ok(summary)
}

pub fn delete_profile(agent: &str, name: String) -> Result<(), String> {
    let _guard = store_lock()?;
    let mut store = load_store()?;
    let before = store.profiles.len();
    store
        .profiles
        .retain(|p| !(p.agent == agent && p.name == name));
    if store.profiles.len() == before {
        return Err(format!("未找到配置 “{name}”"));
    }
    save_store(&store)
}

// ---------------- 快照应用（暂存式回滚） ----------------

#[cfg(target_os = "macos")]
fn clone_config_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    let src_c = std::ffi::CString::new(src.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "源路径含 NUL"))?;
    let dst_c = std::ffi::CString::new(dst.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "目标路径含 NUL"))?;
    if unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) } == 0 {
        // clonefile 的 ACL 可能继承自目标目录而非源文件；显式复制全部元数据。
        let flags = 0x07 | (1 << 18) | (1 << 19); // COPYFILE_METADATA | NOFOLLOW
        if unsafe { libc::copyfile(src_c.as_ptr(), dst_c.as_ptr(), std::ptr::null_mut(), flags) }
            == 0
        {
            return Ok(());
        }
        return Err(std::io::Error::last_os_error());
    }
    let error = std::io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EXDEV)
    ) {
        // 不支持 APFS clone 时也必须保留 ACL/xattr，不能降级为只复制字节。
        let flags = 0x0f | (1 << 17) | (1 << 18) | (1 << 19); // COPYFILE_ALL | EXCL | NOFOLLOW
        if unsafe { libc::copyfile(src_c.as_ptr(), dst_c.as_ptr(), std::ptr::null_mut(), flags) }
            == 0
        {
            return Ok(());
        }
        return Err(std::io::Error::last_os_error());
    }
    Err(error)
}

#[cfg(not(target_os = "macos"))]
fn clone_config_file(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    // 本应用当前发布 macOS。其他平台未实现完整元数据复制时安全失败，
    // 不得悄悄降级为 fs::copy 丢失 ACL/xattr。
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Metadata-preserving configuration copy is unavailable on this platform",
    ))
}

/// 从暂存的原文件复制一份 *.bak 用户备份（延续"应用前自动备份"承诺），
/// 返回备份路径，供用户手工比较或恢复使用。
/// 备份名 = 时间戳 + 进程内序号，保证唯一；create_new 创建（0600），
/// 绝不覆盖已存在的同名文件，也不会跟随预置的符号链接。
/// 事务恢复只用私有元数据副本，绝不从这些用户 .bak 降级恢复。
fn backup_from_staged(staged: &Path, path: &Path) -> Result<PathBuf, String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new("."));
    loop {
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let backup = parent.join(format!("{file_name}.bak-{}-{seq}", now_millis()));
        let mut src = fs::File::open(staged).map_err(|e| format!("备份 {:?} 失败: {e}", path))?;
        let mut dst = match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&backup)
        {
            Ok(f) => f,
            // 极端碰撞：换个名字再来
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("备份 {:?} 失败: {e}", path)),
        };
        std::io::copy(&mut src, &mut dst).map_err(|e| {
            let _ = fs::remove_file(&backup);
            format!("备份 {:?} 失败: {e}", path)
        })?;
        dst.sync_all()
            .map_err(|e| format!("持久化备份 {:?} 失败: {e}", backup))?;
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| format!("持久化备份目录 {:?} 失败: {e}", parent))?;
        return Ok(backup);
    }
}

/// 写入新配置文件：先在同目录写满并 fsync 临时文件，再原子 rename 到目标。
/// 写入原始字节：TOML/JSONC/JSON 逐字节还原，保留注释与手写格式，
/// 不做解析/美化（旧版“重新美化输出”行为已弃用）。
fn write_config_file(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录 {:?} 失败: {e}", parent))?;
    }
    atomic_write(path, content)
}

/// 生产调用者已持有配置库 flock；日志落在应用存储，不落在 CLI 目录。
fn apply_snapshot_in(
    root: &Path,
    spec: &[ConfigFile],
    slots: &[(PathBuf, Option<Vec<u8>>)],
    baseline: Option<&[(PathBuf, Option<Vec<u8>>)]>,
) -> Result<(), String> {
    transaction::apply_snapshot_with(root, spec, slots, baseline, write_config_file, |path| {
        #[cfg(not(test))]
        {
            trash::delete_all([path.to_path_buf()])
                .map_err(|e| format!("移入回收站 {:?} 失败: {e}", path))
        }
        // 测试二进制中的配置事务永远不用系统回收站。
        #[cfg(test)]
        {
            fs::remove_file(path).map_err(|e| e.to_string())
        }
    })
}

/// 旧测试 seam：存储与目标同在调用者的 tempfile 内，绝不访问真实配置库。
#[cfg(test)]
fn apply_snapshot_with<W, T>(
    slots: &[(PathBuf, Option<Vec<u8>>)],
    write: W,
    trash_remove: T,
) -> Result<(), String>
where
    W: FnMut(&Path, &[u8]) -> Result<(), String>,
    T: FnMut(&Path) -> Result<(), String>,
{
    let root = slots
        .first()
        .and_then(|(path, _)| path.parent())
        .ok_or("缺少测试目录")?;
    let spec: Vec<_> = slots
        .iter()
        .enumerate()
        .map(|(i, (path, _))| ConfigFile {
            key: i.to_string(),
            label: i.to_string(),
            path: path.clone(),
        })
        .collect();
    transaction::apply_snapshot_with(root, &spec, slots, None, write, trash_remove)
}

#[cfg(test)]
fn apply_snapshot(slots: &[(PathBuf, Option<Vec<u8>>)]) -> Result<(), String> {
    apply_snapshot_with(slots, write_config_file, |path| {
        fs::remove_file(path).map_err(|e| e.to_string())
    })
}

fn profile_slots(
    profile: &Profile,
    spec: &[ConfigFile],
) -> Result<Vec<(PathBuf, Option<Vec<u8>>)>, String> {
    let uncovered: Vec<&str> = match &profile.captured_keys {
        Some(keys) => spec
            .iter()
            .filter(|config| !keys.iter().any(|key| key == &config.key))
            .map(|config| config.key.as_str())
            .collect(),
        None => spec
            .iter()
            .filter(|config| !profile.files.iter().any(|file| file.key == config.key))
            .map(|config| config.key.as_str())
            .collect(),
    };
    if !uncovered.is_empty() {
        return Err(format!(
            "配置 “{}” 的采集范围无法确认，缺少当前配置文件 {}；为避免删除现用配置，已拒绝应用",
            profile.name,
            uncovered.join("、")
        ));
    }

    Ok(spec
        .iter()
        .map(|config| {
            let content = profile
                .files
                .iter()
                .find(|file| file.key == config.key)
                .map(|file| file.content.clone().into_bytes());
            (config.path.clone(), content)
        })
        .collect())
}

/// 直接更新指定 agent 的某个配置文件内容（内联编辑场景）。
/// 对 JSON / JSONC / TOML 做语法校验，原始文本不重排、不丢注释。
///
/// 与快照应用共用同一套写入事务：store_lock 串行化、暂存原文件、
/// 写入前留 .bak、失败自动回滚（见 apply_snapshot）。绝不裸 fs::write——
/// 那样写到一半失败会把原文件截断成残缺内容。
#[cfg(test)]
fn update_config_file_with_spec(
    agent: &str,
    spec: &[ConfigFile],
    key: &str,
    content: &str,
) -> Result<(), String> {
    let entry = spec
        .iter()
        .find(|c| c.key == key)
        .ok_or_else(|| format!("{agent} 没有 key={key} 的配置文件"))?;

    workspace::validate_document(key, content)?;

    apply_snapshot(&[(entry.path.clone(), Some(content.as_bytes().to_vec()))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn snapshot_apply_removes_files_absent_in_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("settings.json");
        let b = dir.path().join("mcp.json");
        fs::write(&a, r#"{"old":true}"#).unwrap();
        fs::write(&b, r#"{"servers":{}}"#).unwrap();

        // 快照中有 a（新内容）、没有 b：b 必须被移除，而不是保留
        apply_snapshot_with(
            &[
                (a.clone(), Some(br#"{"new":true}"#.to_vec())),
                (b.clone(), None),
            ],
            write_config_file,
            |path| fs::remove_file(path).map_err(|e| e.to_string()),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&a).unwrap(),
            r#"{"new":true}"#,
            "原始字节应逐字节写回"
        );
        assert!(!b.exists(), "快照中不存在的文件应被移除");
        let bak = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.starts_with("mcp.json.bak-")
            })
            .count();
        assert_eq!(bak, 1, "移除前必须生成 .bak 备份");
        let staging_leftovers = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("asm-staging"))
            .count();
        assert_eq!(staging_leftovers, 0, "成功后暂存文件应清理");
    }

    #[test]
    fn snapshot_apply_skips_absent_slot_without_file() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("settings.json");
        // 快照与磁盘都没有该文件：无事发生
        apply_snapshot(&[(a.clone(), None)]).unwrap();
        assert!(!a.exists());
    }

    #[test]
    fn store_lock_is_exclusive_across_file_handles() {
        let dir = tempfile::tempdir().unwrap();
        let guard = lock_store_in(dir.path()).unwrap();
        // 另一个文件句柄（模拟另一进程）非阻塞抢锁：必须失败。
        // 探测用 LOCK_NB：阻塞式 flock 会排队成功，断言失去意义
        let f2 = fs::OpenOptions::new()
            .write(true)
            .open(dir.path().join(".store.lock"))
            .unwrap();
        let rc = unsafe { libc::flock(f2.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert!(rc != 0, "持锁期间其他句柄不得获得排他锁");
        // 释放后应可获得。macOS 高并发负载下 close 释放 flock 的可见性
        // 存在毫秒级延迟（实测 ~1/3000 概率瞬时 EWOULDBLOCK，重试即恢复；
        // 生产代码用阻塞式 LOCK_EX 不受影响），故用有界延迟重试探测
        drop(guard);
        let mut acquired = false;
        for _ in 0..20 {
            if unsafe { libc::flock(f2.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                acquired = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(acquired, "锁释放后短时间内必须可获得");
    }

    #[test]
    fn store_lock_refuses_symlinked_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, "x").unwrap();
        std::os::unix::fs::symlink(&victim, &dir.path().join(".store.lock")).unwrap();
        let err = lock_store_in(dir.path()).unwrap_err();
        assert!(err.contains("锁文件"), "{err}");
    }

    #[test]
    fn store_lock_refuses_fifo_lock_file_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".store.lock");
        let c_path = std::ffi::CString::new(lock_path.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        // 无读端的 FIFO：旧实现 write-open 会永久阻塞（卡死所有配置操作），
        // 现在 O_NONBLOCK 下必须立即报错（ENXIO）而非挂起
        let err = lock_store_in(dir.path()).unwrap_err();
        assert!(err.contains("锁文件"), "{err}");
    }

    #[test]
    fn store_lock_refuses_directory_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        // 目录冒充锁文件：write-open 报 EISDIR，必须显性拒绝
        fs::create_dir_all(dir.path().join(".store.lock")).unwrap();
        let err = lock_store_in(dir.path()).unwrap_err();
        assert!(err.contains("锁文件"), "{err}");
    }

    #[test]
    fn snapshot_apply_preserves_external_edit_between_slots() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("settings.json");
        let b = dir.path().join("mcp.json");
        fs::write(&a, "original a").unwrap();
        fs::write(&b, "original b").unwrap();
        let mut writes = 0;
        let error = apply_snapshot_with(
            &[
                (a.clone(), Some(b"new a".to_vec())),
                (b.clone(), Some(b"new b".to_vec())),
            ],
            |path, bytes| {
                writes += 1;
                write_config_file(path, bytes)?;
                if writes == 1 {
                    fs::write(&b, "external edit").unwrap();
                }
                Ok(())
            },
            |_| panic!("no deletion expected"),
        )
        .unwrap_err();
        assert!(error.contains("外部") || error.contains("冲突"), "{error}");
        assert_eq!(fs::read_to_string(&a).unwrap(), "original a");
        assert_eq!(fs::read_to_string(&b).unwrap(), "external edit");
    }

    #[test]
    fn snapshot_apply_rolls_back_on_phase2_write_failure() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("settings.json");
        let b = dir.path().join("mcp.json");
        fs::write(&a, r#"{"keep":"me"}"#).unwrap();
        let mut writes = 0;

        // 第一槽位成功发布新文件，第二槽位注入写入失败，必须回滚全部槽位
        let err = apply_snapshot_with(
            &[
                (a.clone(), Some(br#"{"new":true}"#.to_vec())),
                (b.clone(), Some(br#"{"again":true}"#.to_vec())),
            ],
            |path, content| {
                writes += 1;
                if writes == 2 {
                    Err("注入的写入失败".to_string())
                } else {
                    write_config_file(path, content)
                }
            },
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(err.contains("回滚"), "错误信息应说明已回滚: {err}");
        assert_eq!(
            fs::read_to_string(&a).unwrap(),
            r#"{"keep":"me"}"#,
            "阶段 2 失败后必须经暂存恢复原内容"
        );
        assert!(!b.exists(), "原本不存在的第二槽位必须恢复为不存在");
        let leftovers = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("asm-staging"))
            .count();
        assert_eq!(leftovers, 0, "回滚后不得残留暂存文件");
    }

    #[test]
    fn snapshot_apply_preserves_permissions_on_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("settings.json");
        let b = dir.path().join("mcp.json");
        fs::write(&a, r#"{"mode":"check"}"#).unwrap();
        fs::set_permissions(&a, fs::Permissions::from_mode(0o644)).unwrap();
        let mut writes = 0;

        // 阶段 2 失败 → 私有暂存副本原子改回，原权限必须恢复
        apply_snapshot_with(
            &[
                (a.clone(), Some(br#"{"new":true}"#.to_vec())),
                (b, Some(br#"{"again":true}"#.to_vec())),
            ],
            |path, content| {
                writes += 1;
                if writes == 2 {
                    Err("注入的写入失败".to_string())
                } else {
                    write_config_file(path, content)
                }
            },
            |_| Ok(()),
        )
        .unwrap_err();
        let mode = fs::symlink_metadata(&a).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "回滚必须恢复原权限（rename 保留属性）");
        assert_eq!(fs::read_to_string(&a).unwrap(), r#"{"mode":"check"}"#);
    }

    #[test]
    fn snapshot_apply_surfaces_capture_errors() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("settings.json");
        fs::write(&a, r#"{"keep":"me"}"#).unwrap();
        // 第二个槽位的父级是普通文件 → symlink_metadata 报 ENOTDIR。
        // 这是"读取失败"，绝不能当成"不存在"静默放行
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, "x").unwrap();
        let bad = blocker.join("child").join("default");

        let err = apply_snapshot(&[
            (a.clone(), Some(br#"{"new":true}"#.to_vec())),
            (bad, Some(br#"{}"#.to_vec())),
        ])
        .unwrap_err();
        assert!(err.contains("无法读取"), "捕获失败必须显性化: {err}");
        assert_eq!(
            fs::read_to_string(&a).unwrap(),
            r#"{"keep":"me"}"#,
            "严格捕获必须在任何修改前中止"
        );
    }

    #[test]
    fn tighten_store_permissions_hardens_legacy_store() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let store = dir.path().join("profiles.json");
        fs::write(&store, r#"{"profiles":[]}"#).unwrap();
        fs::set_permissions(&store, fs::Permissions::from_mode(0o644)).unwrap();

        tighten_store_permissions(&store).unwrap();
        let dmode = fs::symlink_metadata(dir.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let fmode = fs::symlink_metadata(&store).unwrap().permissions().mode() & 0o777;
        assert_eq!(dmode, 0o700, "目录应收紧为 0700");
        assert_eq!(fmode, 0o600, "配置库应收紧为 0600");

        // 文件不存在：不报错
        let absent = dir.path().join("missing.json");
        tighten_store_permissions(&absent).unwrap();
    }

    #[test]
    fn tighten_refuses_symlinked_store_without_chmodding_target() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        fs::write(&real, r#"{"profiles":[]}"#).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o644)).unwrap();
        let link = dir.path().join("profiles.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let err = tighten_store_permissions(&link).unwrap_err();
        assert!(err.contains("符号链接"), "{err}");
        let mode = fs::symlink_metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "不得跟随符号链接 chmod 目标");
    }

    #[test]
    fn tighten_refuses_non_regular_store_file() {
        let dir = tempfile::tempdir().unwrap();
        // 目录冒充配置库：拒绝，绝不能进入读取路径（FIFO 同属此分支）
        let as_dir = dir.path().join("profiles.json");
        fs::create_dir_all(&as_dir).unwrap();
        let err = tighten_store_permissions(&as_dir).unwrap_err();
        assert!(err.contains("不是普通文件"), "{err}");
    }

    #[test]
    fn tighten_refuses_symlinked_store_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let real_dir = tmp.path().join("real");
        fs::create_dir_all(&real_dir).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();

        let err = tighten_store_permissions(&link.join("profiles.json")).unwrap_err();
        assert!(err.contains("不是真实目录"), "{err}");
    }

    #[test]
    fn read_store_file_no_follow_and_type_check() {
        let dir = tempfile::tempdir().unwrap();
        // 普通文件正常读取
        let ok = dir.path().join("ok.json");
        fs::write(&ok, r#"{"a":1}"#).unwrap();
        assert_eq!(read_store_file(&ok).unwrap().as_deref(), Some(r#"{"a":1}"#));
        // 不存在 → None
        assert!(read_store_file(&dir.path().join("missing.json"))
            .unwrap()
            .is_none());
        // 符号链接 → O_NOFOLLOW 拒绝，不得跟随读取目标
        let real = dir.path().join("real.json");
        fs::write(&real, "secret").unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(read_store_file(&link).is_err());
        // 目录即使能打开也必须被 fd 元数据复核拦下
        let adir = dir.path().join("adir");
        fs::create_dir_all(&adir).unwrap();
        let err = read_store_file(&adir).unwrap_err();
        assert!(err.contains("不是普通文件"), "{err}");
    }

    #[test]
    fn snapshot_apply_cleanup_failure_rolls_back_finalized_slot() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("settings.json");
        let b = dir.path().join("mcp.json");
        fs::write(&a, r#"{"orig":"a"}"#).unwrap();
        fs::write(&b, r#"{"orig":"b"}"#).unwrap();
        // b 的回收站操作失败时，a 的暂存必须仍完整，不能降级为 .bak 恢复。
        let err = apply_snapshot_with(
            &[
                (a.clone(), Some(br#"{"new":true}"#.to_vec())),
                (b.clone(), None),
            ],
            write_config_file,
            |_| Err("注入的收尾失败".into()),
        )
        .unwrap_err();
        assert!(err.contains("收尾失败") && err.contains("回滚"), "{err}");

        // 所有槽位共同提交前都保留暂存；失败时完整恢复原文件。
        assert_eq!(
            fs::read_to_string(&a).unwrap(),
            r#"{"orig":"a"}"#,
            "先发布的槽位仍必须从完整暂存恢复"
        );
        assert!(b.exists(), "未收尾槽位必须经暂存恢复");
        assert_eq!(fs::read_to_string(&b).unwrap(), r#"{"orig":"b"}"#);
    }

    #[test]
    fn snapshot_apply_rolls_back_when_trash_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, r#"{"keep":"me"}"#).unwrap();

        let err = apply_snapshot_with(&[(path.clone(), None)], write_config_file, |_| {
            Err("注入的回收站失败".to_string())
        })
        .unwrap_err();
        assert!(err.contains("回滚"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"keep":"me"}"#);
    }

    #[test]
    fn snapshot_apply_cleanup_rollback_preserves_original_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("settings.json");
        let b = dir.path().join("mcp.json");
        fs::write(&a, r#"{"orig":"a"}"#).unwrap();
        fs::write(&b, r#"{"orig":"b"}"#).unwrap();
        // a 用非常规权限 0400：它先发布，随后 b 的回收站操作失败，
        // a 必须从完整暂存恢复，不能依赖只保字节的 .bak。
        fs::set_permissions(&a, fs::Permissions::from_mode(0o400)).unwrap();
        let err = apply_snapshot_with(
            &[
                (a.clone(), Some(br#"{"new":true}"#.to_vec())),
                (b.clone(), None),
            ],
            write_config_file,
            |_| Err("注入的收尾失败".into()),
        )
        .unwrap_err();
        assert!(err.contains("收尾失败") && err.contains("回滚"), "{err}");

        // .bak 固定 0600，完整暂存恢复不得把原来的 0400 变成 0600。
        let mode = fs::symlink_metadata(&a).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o400, "完整暂存恢复必须保留原权限");
        assert_eq!(fs::read_to_string(&a).unwrap(), r#"{"orig":"a"}"#);
        // 未收尾槽位经暂存 rename 恢复，权限随 inode 保留
        assert_eq!(fs::read_to_string(&b).unwrap(), r#"{"orig":"b"}"#);
    }

    #[test]
    fn snapshot_apply_refuses_symlink_target() {
        let dir = tempfile::tempdir().unwrap();
        // dotfiles 场景：配置文件是指向仓库的符号链接
        let real = dir.path().join("real-settings.json");
        fs::write(&real, r#"{"dotfile":true}"#).unwrap();
        let link = dir.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let err = apply_snapshot(&[(link.clone(), Some(br#"{"new":true}"#.to_vec()))]).unwrap_err();
        assert!(err.contains("符号链接"), "{err}");
        assert!(
            link.symlink_metadata().unwrap().file_type().is_symlink(),
            "链接必须保持，不得被悄悄替换成普通文件"
        );
        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            r#"{"dotfile":true}"#,
            "链接目标必须完好"
        );
    }

    #[test]
    fn snapshot_apply_refuses_non_file_target() {
        let dir = tempfile::tempdir().unwrap();
        // 目标路径是目录：严格捕获阶段即拒绝，不得替换
        let target = dir.path().join("adir");
        fs::create_dir_all(&target).unwrap();

        let err = apply_snapshot(&[(target.clone(), Some(br#"{"x":1}"#.to_vec()))]).unwrap_err();
        assert!(err.contains("不是普通文件"), "{err}");
        assert!(target.is_dir(), "目录应保持原样");
    }

    #[test]
    fn atomic_write_is_private_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("profiles.json");
        atomic_write(&target, br#"{"ok":true}"#).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"ok":true}"#);
        let mode = fs::symlink_metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "配置库文件必须是 0600");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "不得残留临时文件");
    }

    #[test]
    fn read_config_raw_distinguishes_absent_from_present() {
        let dir = tempfile::tempdir().unwrap();
        // 不存在 → Ok(None)
        let absent = dir.path().join("missing.json");
        assert!(read_config_raw(&absent).unwrap().is_none());
        // 有字节 → Some（逐字节，不做解析）；含注释的 JSONC 也原样保留
        let ok = dir.path().join("ok.jsonc");
        let raw = "// comment\n{\"a\":1}\n";
        fs::write(&ok, raw).unwrap();
        assert_eq!(read_config_raw(&ok).unwrap(), Some(raw.as_bytes().to_vec()));
    }

    #[test]
    fn snapshot_preserves_comments_and_formatting_byte_for_byte() {
        // 原始字节快照：应用后内容与快照完全一致（不重排字段、不丢注释）
        let dir = tempfile::tempdir().unwrap();
        let toml = dir.path().join("config.toml");
        fs::write(&toml, "old = true\n").unwrap();
        // 带注释、非美化缩进的 TOML
        let content = "# my config\nmodel = \"gpt\"   # trailing\n[nested]\nx=1\n";
        apply_snapshot(&[(toml.clone(), Some(content.as_bytes().to_vec()))]).unwrap();
        assert_eq!(
            fs::read_to_string(&toml).unwrap(),
            content,
            "字节必须完全一致"
        );
    }

    #[test]
    fn legacy_pi_snapshot_without_settings_scope_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("settings.json");
        fs::write(&current, r#"{"current":true}"#).unwrap();
        let profile = Profile {
            name: "legacy".to_string(),
            agent: "pi".to_string(),
            created_at: 1,
            files: vec![ConfigSnapshot {
                key: "config.json".to_string(),
                content: r#"{"old":true}"#.to_string(),
            }],
            captured_keys: None,
        };
        let spec = vec![ConfigFile {
            key: "settings.json".to_string(),
            label: "settings.json".to_string(),
            path: current.clone(),
        }];

        let err = profile_slots(&profile, &spec).unwrap_err();
        assert!(err.contains("采集范围"), "{err}");
        assert_eq!(fs::read_to_string(current).unwrap(), r#"{"current":true}"#);
    }

    #[test]
    fn legacy_pi_empty_or_missing_files_does_not_gain_qoder_scope() {
        let dir = tempfile::tempdir().unwrap();
        let spec = vec![ConfigFile {
            key: "settings.json".into(),
            label: "settings.json".into(),
            path: dir.path().join("settings.json"),
        }];
        for raw in [
            r#"{"name":"legacy","agent":"pi","createdAt":1,"files":[]}"#,
            r#"{"name":"legacy","agent":"pi","createdAt":1}"#,
        ] {
            let profile = serde_json::from_str::<RawProfile>(raw)
                .unwrap()
                .into_profile();
            assert!(
                profile_slots(&profile, &spec).is_err(),
                "未采集过 Pi settings 的快照不得获得删除权限: {raw}"
            );
        }
    }

    #[test]
    fn captured_scope_distinguishes_known_absent_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let profile = Profile {
            name: "known-absent".to_string(),
            agent: "pi".to_string(),
            created_at: 1,
            files: Vec::new(),
            captured_keys: Some(vec!["settings.json".to_string()]),
        };
        let spec = vec![ConfigFile {
            key: "settings.json".to_string(),
            label: "settings.json".to_string(),
            path: path.clone(),
        }];

        let slots = profile_slots(&profile, &spec).unwrap();
        assert_eq!(slots, vec![(path, None)]);
    }

    #[test]
    fn inline_update_validates_and_uses_snapshot_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, r#"{"before":true}"#).unwrap();
        let spec = vec![ConfigFile {
            key: "settings.json".to_string(),
            label: "settings.json".to_string(),
            path: path.clone(),
        }];

        update_config_file_with_spec("test", &spec, "settings.json", r#"{"after":true}"#).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"after":true}"#);
        assert!(fs::read_dir(dir.path()).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("settings.json.bak-")
        }));

        let err =
            update_config_file_with_spec("test", &spec, "settings.json", "{broken").unwrap_err();
        assert!(err.contains("JSON 语法错误"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"after":true}"#);
    }

    #[test]
    fn inline_update_interrupted_write_preserves_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let original = br#"{"origin":"keep-me"}"#;
        fs::write(&path, original).unwrap();
        // 文件大小限制是进程级状态，只在独立测试子进程中设置。
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "agents::profiles::tests::inline_update_file_limit_child",
                "--nocapture",
            ])
            .env("ASM_TEST_FILE_LIMIT_TARGET", &path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn inline_update_file_limit_child() {
        let Ok(target) = std::env::var("ASM_TEST_FILE_LIMIT_TARGET") else {
            return;
        };
        let path = PathBuf::from(target);
        let original = fs::read(&path).unwrap();
        // 日志与原文件准备完成后再限制写入大小，真正覆盖原子写的半写失败。
        let content = serde_json::json!({"long": "x".repeat(300)}).to_string();
        let error = apply_snapshot_with(
            &[(path.clone(), Some(content.into_bytes()))],
            |target, bytes| {
                unsafe {
                    libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                    let limit = libc::rlimit {
                        rlim_cur: 64,
                        rlim_max: 64,
                    };
                    assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
                }
                write_config_file(target, bytes)
            },
            |_| panic!("no deletion expected"),
        )
        .unwrap_err();
        assert!(error.contains("写入临时文件"), "{error}");
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn legacy_profile_migrates_to_qoder_files() {
        // 旧格式（settings/mcp/modelsDefault 字段、无 files/agent）→ agent=qoder + files
        let raw = r#"{"profiles":[{"name":"old","createdAt":1,"settings":{"a":1},"mcp":{"servers":{}}}]}"#;
        let store: RawStore = serde_json::from_str(raw).unwrap();
        let profiles: Vec<Profile> = store
            .profiles
            .into_iter()
            .map(RawProfile::into_profile)
            .collect();
        assert_eq!(profiles.len(), 1);
        let p = &profiles[0];
        assert_eq!(p.agent, "qoder", "旧格式应迁移为 qoder");
        assert!(p.files.iter().any(|f| f.key == "settings.json"));
        assert!(p.files.iter().any(|f| f.key == "mcp.json"));
        assert!(
            !p.files.iter().any(|f| f.key == ".models/default"),
            "未提供的旧字段不应凭空生成 file"
        );
        let captured: Vec<&str> = p
            .captured_keys
            .as_deref()
            .unwrap()
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(
            captured,
            vec!["settings.json", "mcp.json", ".models/default"],
            "旧 Qoder 字段格式的完整采集范围是已知的"
        );
    }

    #[test]
    fn new_format_profile_kept_as_is() {
        // 新格式（已含 files）：不走迁移，原样保留
        let raw = r#"{"profiles":[{"name":"n","agent":"codex","createdAt":2,"files":[{"key":"config.toml","content":"x=1\n"}],"capturedKeys":["config.toml"]}]}"#;
        let store: RawStore = serde_json::from_str(raw).unwrap();
        let p = store.profiles.into_iter().next().unwrap().into_profile();
        assert_eq!(p.agent, "codex");
        assert_eq!(p.files.len(), 1);
        assert_eq!(p.files[0].key, "config.toml");
        assert_eq!(p.files[0].content, "x=1\n");
        assert_eq!(p.captured_keys, Some(vec!["config.toml".to_string()]));
    }
}
