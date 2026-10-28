//! 持久配置事务。调用者持有 store flock；CLI 不协作，故每次发布/恢复都复核版本。
//! 日志只授予实际 config_spec 中的目标权限；暂存路径由目标与事务 ID 推导。

use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Component;

type Slots = [(PathBuf, Option<Vec<u8>>)];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Version {
    dev: u64,
    ino: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    len: u64,
    mtime: i64,
    mtime_ns: i64,
    ctime: i64,
    ctime_ns: i64,
    sha256: String,
    attributes: String,
}

impl Version {
    fn metadata(m: &fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            len: m.len(),
            mtime: m.mtime(),
            mtime_ns: m.mtime_nsec(),
            ctime: m.ctime(),
            ctime_ns: m.ctime_nsec(),
            sha256: String::new(),
            attributes: String::new(),
        }
    }

    fn same_content_and_metadata(&self, other: &Self) -> bool {
        self.sha256 == other.sha256
            && self.attributes == other.attributes
            && self.mode == other.mode
            && self.uid == other.uid
            && self.gid == other.gid
            && self.len == other.len
            && self.mtime == other.mtime
            && self.mtime_ns == other.mtime_ns
    }

    // rename 会改变 ctime；其余身份、内容、权限、mtime 必须一致。
    fn renamed_eq(&self, other: &Self) -> bool {
        let mut expected = self.clone();
        expected.ctime = other.ctime;
        expected.ctime_ns = other.ctime_ns;
        expected == *other
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Preparing,
    Publishing,
    Committed,
    RolledBack,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Slot {
    key: String,
    target: PathBuf,
    staging: PathBuf,
    before: Option<Version>,
    after_hash: Option<String>,
    original: Option<Version>,
    undo: Option<Version>,
    published: Option<Version>,
    armed: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    id: String,
    phase: Phase,
    slots: Vec<Slot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    Prepared,
    BeforePublish(usize),
    Published(usize),
    BeforeCommit,
    Committed,
    Cleanup(usize),
    BeforeRestore(usize),
    Restored(usize),
}

#[cfg(test)]
thread_local! {
    static HOOK: std::cell::RefCell<Option<Box<dyn FnMut(Event) -> Result<(), String>>>> =
        std::cell::RefCell::new(None);
}

fn checkpoint(event: Event) -> Result<(), String> {
    #[cfg(test)]
    return HOOK.with(|hook| match hook.borrow_mut().as_mut() {
        Some(hook) => hook(event),
        None => Ok(()),
    });
    #[cfg(not(test))]
    {
        let _ = event;
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn with_hook<R>(
    hook: impl FnMut(Event) -> Result<(), String> + 'static,
    run: impl FnOnce() -> R,
) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            HOOK.with(|h| *h.borrow_mut() = None);
        }
    }
    HOOK.with(|h| {
        assert!(h.borrow().is_none());
        *h.borrow_mut() = Some(Box::new(hook));
    });
    let _reset = Reset;
    run()
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// 只接受干净的绝对路径。macOS 的系统 /var、/tmp 别名在可信输入端展开，
/// 其余任何层级的符号链接（包括配置目录、暂存目录）一律拒绝。
fn safe_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || path
            .as_os_str()
            .as_encoded_bytes()
            .split(|b| *b == b'/')
            .any(|s| s == b"." || s == b"..")
    {
        return Err(format!("配置事务拒绝非绝对路径或路径穿越：{path:?}"));
    }
    let mut path = path.to_path_buf();
    #[cfg(target_os = "macos")]
    for alias in ["/var", "/tmp", "/etc"] {
        if let Ok(rest) = path.strip_prefix(alias) {
            let actual = Path::new("/private").join(&alias[1..]);
            if fs::read_link(alias)
                .is_ok_and(|link| link == actual || link == Path::new(&format!("private{alias}")))
            {
                path = actual.join(rest);
            }
            break;
        }
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!("配置事务拒绝符号链接：{current:?}"))
            }
            Ok(m) if current != path && !m.is_dir() => {
                return Err(format!("无法读取配置路径：{current:?} 不是目录"))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("无法读取配置路径 {current:?}：{e}")),
        }
    }
    Ok(path)
}

fn io_error(path: &Path, e: std::io::Error) -> String {
    format!("配置事务文件操作 {path:?} 失败：{e}")
}

fn sync_dir(path: &Path) -> Result<(), String> {
    let path = safe_path(path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(&path)
        .map_err(|e| io_error(&path, e))?;
    file.sync_all().map_err(|e| io_error(&path, e))
}

fn create_dirs(path: &Path) -> Result<(), String> {
    let path = safe_path(path)?;
    if path.is_dir() {
        return Ok(());
    }
    let parent = path.parent().ok_or("配置目录没有父目录")?;
    create_dirs(parent)?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .map_err(|e| io_error(&path, e))?;
    sync_dir(&path)?;
    sync_dir(parent)
}

fn private_dir(path: &Path) -> Result<(), String> {
    let m = fs::symlink_metadata(path).map_err(|e| io_error(path, e))?;
    if !m.is_dir()
        || m.file_type().is_symlink()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o700
    {
        return Err(format!(
            "事务目录必须为当前用户所有的 0700 真实目录：{path:?}"
        ));
    }
    Ok(())
}

fn journal_path(root: &Path) -> Result<PathBuf, String> {
    let root = safe_path(root)?;
    let dir = root.join("config-transactions");
    if fs::symlink_metadata(&dir).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        create_dirs(&dir)?;
    }
    safe_path(&dir)?;
    private_dir(&dir)?;
    Ok(dir.join("pending.json"))
}

fn capture(path: &Path) -> Result<Option<Version>, String> {
    let path = safe_path(path)?;
    let mut file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error(&path, e)),
    };
    let before = file.metadata().map_err(|e| io_error(&path, e))?;
    if !before.is_file() || before.nlink() != 1 {
        return Err(format!("配置事务目标不是普通文件或存在硬链接：{path:?}"));
    }
    let mut result = Version::metadata(&before);
    let mut hash = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).map_err(|e| io_error(&path, e))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    let attributes = attribute_digest(file.as_raw_fd()).map_err(|e| io_error(&path, e))?;
    let after = file.metadata().map_err(|e| io_error(&path, e))?;
    let named = fs::symlink_metadata(&path).map_err(|e| io_error(&path, e))?;
    if result != Version::metadata(&after) || result != Version::metadata(&named) {
        return Err(format!("配置文件在读取期间被外部修改：{path:?}"));
    }
    result.sha256 = format!("{:x}", hash.finalize());
    result.attributes = attributes;
    Ok(Some(result))
}

#[cfg(target_os = "macos")]
mod acl {
    use std::ffi::c_void;
    extern "C" {
        pub fn acl_get_fd(fd: libc::c_int) -> *mut c_void;
        pub fn acl_to_text(acl: *mut c_void, len: *mut libc::ssize_t) -> *mut libc::c_char;
        pub fn acl_free(object: *mut c_void) -> libc::c_int;
    }
    pub struct Owned(pub *mut c_void);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                acl_free(self.0);
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn attribute_digest(fd: libc::c_int) -> std::io::Result<String> {
    fn buffer(size: isize) -> std::io::Result<Vec<u8>> {
        if size < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if size > 16 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Extended metadata exceeds transaction limit",
            ));
        }
        Ok(vec![0; size as usize])
    }
    let mut names = buffer(unsafe { libc::flistxattr(fd, std::ptr::null_mut(), 0, 0) })?;
    let length = unsafe { libc::flistxattr(fd, names.as_mut_ptr().cast(), names.len(), 0) };
    if length < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if length as usize > names.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "Extended metadata changed while reading",
        ));
    }
    names.truncate(length as usize);
    let mut names = names
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    names.sort();
    let mut hash = Sha256::new();
    for name in names {
        let c_name = std::ffi::CString::new(name).unwrap();
        let mut value =
            buffer(unsafe { libc::fgetxattr(fd, c_name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) })?;
        let length = unsafe {
            libc::fgetxattr(
                fd,
                c_name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
                0,
                0,
            )
        };
        if length < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if length as usize > value.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Extended metadata changed while reading",
            ));
        }
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update((length as u64).to_le_bytes());
        hash.update(&value[..length as usize]);
    }
    let acl = acl::Owned(unsafe { acl::acl_get_fd(fd) });
    if acl.0.is_null() {
        let error = std::io::Error::last_os_error();
        // fd 已经确认是普通文件；macOS 用 ENOENT 表示该 inode 没有扩展 ACL。
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(format!("{:x}", hash.finalize()));
        }
        return Err(error);
    }
    let mut len = 0;
    let text = acl::Owned(unsafe { acl::acl_to_text(acl.0, &mut len) }.cast());
    if text.0.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    hash.update(unsafe { std::slice::from_raw_parts(text.0.cast::<u8>(), len as usize) });
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(not(target_os = "macos"))]
fn attribute_digest(_fd: libc::c_int) -> std::io::Result<String> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Metadata-preserving configuration transactions require macOS",
    ))
}

fn matches_renamed(expected: &Option<Version>, actual: &Option<Version>) -> bool {
    match (expected, actual) {
        (Some(a), Some(b)) => a.renamed_eq(b),
        (None, None) => true,
        _ => false,
    }
}

fn conflict(path: &Path) -> String {
    format!("配置事务冲突：{path:?} 已被外部修改；保留当前内容和事务恢复材料，请人工比较处理后重试")
}

fn holder(target: &Path, id: &str, index: usize) -> PathBuf {
    target.with_file_name(format!(
        ".{}.asm-staging-{id}-{index}",
        target.file_name().unwrap().to_string_lossy()
    ))
}

fn persist(path: &Path, journal: &Journal) -> Result<(), String> {
    // atomic_write 同时持久化文件与 rename 的父目录。
    atomic_write(
        path,
        &serde_json::to_vec(journal).map_err(|e| e.to_string())?,
    )
}

fn validate(journal: &Journal, spec: &[ConfigFile]) -> Result<(), String> {
    if journal.version != 1
        || journal.id.len() != 64
        || !journal
            .id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || journal.slots.is_empty()
        || journal.slots.len() > spec.len()
    {
        return Err("配置事务日志版本、身份或槽位数量无效".into());
    }
    let mut seen = HashSet::new();
    for (i, slot) in journal.slots.iter().enumerate() {
        // 从 config_spec 生成允许列表，不使用日志自行声明的目录范围。
        let entry = spec
            .iter()
            .find(|f| f.key == slot.key && f.path.as_os_str() == slot.target.as_os_str())
            .ok_or("配置事务日志目标不在当前 config_spec 允许列表，已拒绝恢复")?;
        let target = if journal.phase == Phase::Publishing {
            safe_path(&entry.path)?
        } else {
            // 仅清理同目录暂存，不读取或改写可能已被外部替换的目标叶节点。
            safe_path(entry.path.parent().ok_or("配置事务目标缺少父目录")?)?
                .join(entry.path.file_name().ok_or("配置事务目标缺少文件名")?)
        };
        if !seen.insert(target.clone())
            || slot.staging.as_os_str() != holder(&target, &journal.id, i).as_os_str()
        {
            return Err("配置事务日志含重复目标或伪造的暂存路径".into());
        }
        safe_path(&slot.staging)?;
        if fs::symlink_metadata(&slot.staging).is_ok() {
            private_dir(&slot.staging)?;
        }
        staging_files(&slot.staging)?;
        if journal.phase != Phase::Preparing {
            if slot.before.is_some() != slot.original.is_some()
                || slot.before.is_some() != slot.undo.is_some()
                || slot.after_hash.is_some() != slot.published.is_some()
                || slot.before.as_ref().map(|v| &v.sha256)
                    != slot.original.as_ref().map(|v| &v.sha256)
                || slot.before.as_ref().map(|v| &v.sha256) != slot.undo.as_ref().map(|v| &v.sha256)
                || slot.after_hash.as_ref() != slot.published.as_ref().map(|v| &v.sha256)
            {
                return Err("配置事务日志版本指纹不一致".into());
            }
        } else if slot.armed {
            return Err("准备中的配置事务不得含已发布槽位".into());
        }
    }
    Ok(())
}

/// atomic_write 在 rename 前崩溃可能留下其私有临时文件。仅接受固定名称和
/// 严格数字后缀，且逐项拒绝链接/特殊文件；目录里有未知材料时保留全部材料。
fn staging_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_error(dir, e)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_error(dir, e))?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("事务暂存目录包含未知文件名")?;
        let temporary = name.strip_prefix(".publish.tmp-").is_some_and(|suffix| {
            let parts: Vec<_> = suffix.split('-').collect();
            parts.len() == 2
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
        });
        if !matches!(name, "original" | "undo" | "publish") && !temporary {
            return Err(format!("事务暂存目录包含未知文件，已保留：{dir:?}"));
        }
        let file = dir.join(name);
        capture(&file)?.ok_or("事务暂存文件在检查期间消失")?;
        files.push(file);
    }
    Ok(files)
}

/// 清理只发生在持久终态之后。绝不递归删除，绝不消费旧 .bak。
fn cleanup(path: &Path, journal: &Journal) -> Result<(), String> {
    for (i, slot) in journal.slots.iter().enumerate() {
        checkpoint(Event::Cleanup(i))?;
        safe_path(&slot.staging)?;
        match fs::symlink_metadata(&slot.staging) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_error(&slot.staging, e)),
            Ok(_) => private_dir(&slot.staging)?,
        }
        for file in staging_files(&slot.staging)? {
            fs::remove_file(&file).map_err(|e| io_error(&file, e))?;
        }
        sync_dir(&slot.staging)?;
        fs::remove_dir(&slot.staging).map_err(|e| io_error(&slot.staging, e))?;
        sync_dir(slot.staging.parent().unwrap())?;
    }
    fs::remove_file(path).map_err(|e| io_error(path, e))?;
    sync_dir(path.parent().unwrap())
}

fn recover_journal(path: &Path, journal: &mut Journal, spec: &[ConfigFile]) -> Result<(), String> {
    validate(journal, spec)?;
    if journal.phase == Phase::Preparing {
        return cleanup(path, journal);
    }
    if matches!(journal.phase, Phase::Committed | Phase::RolledBack) {
        // 上次可能在 marker rename 后、目录 fsync 失败时退出；先确保终态持久化。
        sync_file(path)?;
        sync_dir(path.parent().unwrap())?;
        return cleanup(path, journal);
    }
    let mut errors = Vec::new();
    for (i, slot) in journal.slots.iter().enumerate().rev() {
        let result = (|| {
            checkpoint(Event::BeforeRestore(i))?;
            let target = safe_path(&slot.target)?;
            let current = capture(&target)?;
            if current == slot.before || (slot.armed && matches_renamed(&slot.undo, &current)) {
                // 包含 rename 已完成、恢复进度尚未来得及持久化的情况。
                sync_dir(target.parent().unwrap())?;
                return Ok(());
            }
            if !slot.armed || !matches_renamed(&slot.published, &current) {
                return Err(conflict(&target));
            }
            if let Some(expected) = &slot.undo {
                let undo = slot.staging.join("undo");
                let actual = capture(&undo)?;
                if !actual.as_ref().is_some_and(|v| expected.renamed_eq(v)) {
                    return Err(format!(
                        "原文件恢复副本缺失或被修改：{undo:?}；保留恢复材料"
                    ));
                }
                // 紧贴 rename 再读一次，不把先前确认当成永久覆盖许可。
                if capture(&target)? != current {
                    return Err(conflict(&target));
                }
                fs::rename(&undo, &target).map_err(|e| io_error(&target, e))?;
                sync_dir(&slot.staging)?;
            } else {
                if capture(&target)? != current {
                    return Err(conflict(&target));
                }
                if current.is_some() {
                    fs::remove_file(&target).map_err(|e| io_error(&target, e))?;
                }
            }
            sync_dir(target.parent().unwrap())?;
            checkpoint(Event::Restored(i))?;
            Ok::<_, String>(())
        })();
        if let Err(e) = result {
            errors.push(e);
        }
    }
    // 恢复后续槽位时，CLI 仍可能修改先前恢复好的槽位；终态前再整组复核。
    for slot in &journal.slots {
        match capture(&slot.target) {
            Ok(current)
                if current == slot.before
                    || (slot.armed && matches_renamed(&slot.undo, &current)) => {}
            Ok(_) => errors.push(conflict(&slot.target)),
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(format!("{}；恢复日志：{path:?}", errors.join("；")));
    }
    journal.phase = Phase::RolledBack;
    persist(path, journal)?;
    cleanup(path, journal)
}

pub(super) fn recover_in(root: &Path, spec: &[ConfigFile]) -> Result<(), String> {
    let path = journal_path(root)?;
    if let Ok(m) = fs::symlink_metadata(&path) {
        if !m.is_file()
            || m.file_type().is_symlink()
            || m.nlink() != 1
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o777 != 0o600
        {
            return Err(format!(
                "配置事务日志必须是当前用户所有的 0600 普通文件：{path:?}"
            ));
        }
    }
    let Some(raw) = read_store_file(&path)? else {
        return Ok(());
    };
    let mut journal: Journal = serde_json::from_str(&raw)
        .map_err(|_| format!("配置事务日志损坏：{path:?}；未修改任何目标，请人工处理"))?;
    recover_journal(&path, &mut journal, spec)
}

fn sync_file(path: &Path) -> Result<(), String> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| io_error(path, e))
}

pub(super) fn apply_snapshot_with<W, T>(
    root: &Path,
    spec: &[ConfigFile],
    slots: &Slots,
    baseline: Option<&Slots>,
    mut write: W,
    mut trash_remove: T,
) -> Result<(), String>
where
    W: FnMut(&Path, &[u8]) -> Result<(), String>,
    T: FnMut(&Path) -> Result<(), String>,
{
    recover_in(root, spec)?;
    if slots.is_empty() {
        return Ok(());
    }
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = digest(
        format!(
            "{}-{:?}-{}",
            std::process::id(),
            SystemTime::now(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
        .as_bytes(),
    );
    let mut journal = Journal {
        version: 1,
        id,
        phase: Phase::Preparing,
        slots: Vec::new(),
    };
    for (i, (path, content)) in slots.iter().enumerate() {
        let entry = spec
            .iter()
            .find(|f| f.path == *path)
            .ok_or("配置事务目标不在 config_spec 允许列表")?;
        let target = safe_path(path)?;
        let before = capture(&target)?;
        if let Some(baseline) = baseline {
            let (_, bytes) = baseline
                .iter()
                .find(|(p, _)| p == path)
                .ok_or("缺少配置的预期原始版本")?;
            if bytes.as_deref().map(digest) != before.as_ref().map(|v| v.sha256.clone()) {
                return Err(conflict(path));
            }
        }
        journal.slots.push(Slot {
            key: entry.key.clone(),
            target: path.clone(),
            staging: holder(&target, &journal.id, i),
            before,
            after_hash: content.as_deref().map(digest),
            original: None,
            undo: None,
            published: None,
            armed: false,
        });
    }
    validate(&journal, spec)?;
    let journal_path = journal_path(root)?;
    persist(&journal_path, &journal)?;
    // 准备阶段的日志已经落盘，任何准备中断只会留下可识别、可清理的私有材料。
    let result = (|| {
        for (i, slot) in journal.slots.iter_mut().enumerate() {
            let target = safe_path(&slot.target)?;
            create_dirs(target.parent().unwrap())?;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&slot.staging)
                .map_err(|e| io_error(&slot.staging, e))?;
            sync_dir(slot.staging.parent().unwrap())?;
            if capture(&target)? != slot.before {
                return Err(conflict(&target));
            }
            if slot.before.is_some() {
                let original = slot.staging.join("original");
                clone_config_file(&target, &original).map_err(|e| io_error(&original, e))?;
                let undo = slot.staging.join("undo");
                clone_config_file(&original, &undo).map_err(|e| io_error(&undo, e))?;
                sync_file(&original)?;
                sync_file(&undo)?;
                slot.original = capture(&original)?;
                slot.undo = capture(&undo)?;
                let before = slot.before.as_ref().unwrap();
                if !slot
                    .original
                    .as_ref()
                    .is_some_and(|v| before.same_content_and_metadata(v))
                    || !slot
                        .undo
                        .as_ref()
                        .is_some_and(|v| before.same_content_and_metadata(v))
                {
                    return Err(format!(
                        "暂存副本未完整保留原文件内容或元数据：{target:?}；已中止发布"
                    ));
                }
                if capture(&target)? != slot.before {
                    return Err(conflict(&target));
                }
                backup_from_staged(&original, &target)?;
            }
            if let Some(content) = &slots[i].1 {
                let publish = slot.staging.join("publish");
                // 注入写入器只写私有待发布文件，不再获准直接截断目标。
                write(&publish, content)?;
                sync_file(&publish)?;
                slot.published = capture(&publish)?;
                if slot.published.as_ref().map(|v| &v.sha256) != slot.after_hash.as_ref() {
                    return Err("待发布配置内容校验失败".into());
                }
            }
            sync_dir(&slot.staging)?;
        }
        journal.phase = Phase::Publishing;
        persist(&journal_path, &journal)?;
        checkpoint(Event::Prepared)?;
        // 先发布全部新文件，再执行回收站；全部完成以前不删除任何原始暂存。
        let order: Vec<_> = (0..slots.len())
            .filter(|i| slots[*i].1.is_some())
            .chain((0..slots.len()).filter(|i| slots[*i].1.is_none()))
            .collect();
        for i in order {
            journal.slots[i].armed = true;
            persist(&journal_path, &journal)?;
            checkpoint(Event::BeforePublish(i))?;
            let slot = &journal.slots[i];
            let target = safe_path(&slot.target)?;
            if capture(&target)? != slot.before {
                return Err(conflict(&target));
            }
            if slot.after_hash.is_some() {
                let publish = slot.staging.join("publish");
                if capture(&publish)? != slot.published {
                    return Err("待发布配置文件被修改".into());
                }
                if capture(&target)? != slot.before {
                    return Err(conflict(&target));
                }
                fs::rename(&publish, &target).map_err(|e| io_error(&target, e))?;
                sync_dir(&slot.staging)?;
            } else if slot.before.is_some() {
                trash_remove(&target).map_err(|e| format!("收尾失败：{e}"))?;
            }
            sync_dir(target.parent().unwrap())?;
            if !matches_renamed(&slot.published, &capture(&target)?) {
                return Err(conflict(&target));
            }
            checkpoint(Event::Published(i))?;
        }
        checkpoint(Event::BeforeCommit)?;
        for slot in &journal.slots {
            if !matches_renamed(&slot.published, &capture(&slot.target)?) {
                return Err(conflict(&slot.target));
            }
        }
        journal.phase = Phase::Committed;
        // 即使持久化返回错误（例如 rename 成功而目录 fsync 失败），也不能盲目回滚。
        persist(&journal_path, &journal)?;
        Ok::<_, String>(())
    })();
    if let Err(cause) = result {
        // 重新读取磁盘 phase，而非用内存猜测提交记录是否已发布。
        return match recover_in(root, spec) {
            Ok(()) if journal.phase != Phase::Committed => {
                Err(format!("{cause}（已回滚到应用前状态）"))
            }
            Ok(()) => Err(format!("{cause}；提交状态已按持久日志处理，请刷新配置确认")),
            Err(error) => Err(format!("{cause}；恢复未完成：{error}")),
        };
    }
    checkpoint(Event::Committed)?;
    cleanup(&journal_path, &journal)
        .map_err(|e| format!("配置已提交，暂存清理未完成，下次访问将重试：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    struct Fixture {
        tmp: tempfile::TempDir,
        spec: Vec<ConfigFile>,
    }
    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let spec = ["a.json", "b.json", "c.json"]
                .into_iter()
                .map(|key| ConfigFile {
                    key: key.into(),
                    label: key.into(),
                    path: tmp.path().join(key),
                })
                .collect::<Vec<_>>();
            fs::write(&spec[0].path, b"original a").unwrap();
            fs::write(&spec[1].path, b"original b").unwrap();
            Self { tmp, spec }
        }
        fn slots(&self) -> Vec<(PathBuf, Option<Vec<u8>>)> {
            vec![
                (self.spec[0].path.clone(), Some(b"new a".to_vec())),
                (self.spec[1].path.clone(), None),
                (self.spec[2].path.clone(), Some(b"new c".to_vec())),
            ]
        }
        fn apply(&self) -> Result<(), String> {
            apply_snapshot_with(
                self.tmp.path(),
                &self.spec,
                &self.slots(),
                None,
                write_config_file,
                |p| fs::remove_file(p).map_err(|e| e.to_string()),
            )
        }
        fn recover(&self) -> Result<(), String> {
            recover_in(self.tmp.path(), &self.spec)
        }
        fn journal(&self) -> PathBuf {
            self.tmp.path().join("config-transactions/pending.json")
        }
        fn interrupt(&self, at: Event) {
            let result = catch_unwind(AssertUnwindSafe(|| {
                with_hook(
                    move |event| {
                        if event == at {
                            panic!("simulated process interruption at {event:?}");
                        }
                        Ok(())
                    },
                    || self.apply().unwrap(),
                )
            }));
            assert!(result.is_err());
            assert!(
                self.journal().is_file(),
                "interruption must leave a durable journal"
            );
        }
        fn assert_original(&self) {
            assert_eq!(fs::read(&self.spec[0].path).unwrap(), b"original a");
            assert_eq!(fs::read(&self.spec[1].path).unwrap(), b"original b");
            assert!(!self.spec[2].path.exists());
        }
        fn assert_committed(&self) {
            assert_eq!(fs::read(&self.spec[0].path).unwrap(), b"new a");
            assert!(!self.spec[1].path.exists());
            assert_eq!(fs::read(&self.spec[2].path).unwrap(), b"new c");
        }
    }

    #[test]
    fn interrupted_multifile_transactions_recover_at_each_durable_boundary() {
        for event in [
            Event::Prepared,
            Event::BeforePublish(0),
            Event::Published(0),
            Event::Published(2),
            Event::Published(1),
            Event::BeforeCommit,
            Event::Committed,
            Event::Cleanup(1),
        ] {
            let f = Fixture::new();
            f.interrupt(event);
            f.recover().unwrap();
            f.recover().unwrap();
            if matches!(event, Event::Committed | Event::Cleanup(_)) {
                f.assert_committed();
            } else {
                f.assert_original();
            }
            assert!(!f.journal().exists());
            let names = fs::read_dir(f.tmp.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            assert!(!names.iter().any(|n| n.contains("asm-staging")));
            assert_eq!(names.iter().filter(|n| n.contains(".bak-")).count(), 2);
        }
    }

    #[test]
    fn rollback_can_itself_restart_after_every_restored_slot() {
        for event in [
            Event::Restored(2),
            Event::Restored(1),
            Event::Restored(0),
            Event::Cleanup(1),
        ] {
            let f = Fixture::new();
            f.interrupt(Event::BeforeCommit);
            let stopped = catch_unwind(AssertUnwindSafe(|| {
                with_hook(
                    move |e| {
                        if e == event {
                            panic!("interrupted recovery");
                        }
                        Ok(())
                    },
                    || f.recover().unwrap(),
                )
            }));
            assert!(stopped.is_err());
            f.recover().unwrap();
            f.recover().unwrap();
            f.assert_original();
        }
    }

    #[test]
    fn ordinary_publish_failure_rolls_back_and_committed_cleanup_failure_only_retries_cleanup() {
        let f = Fixture::new();
        let error = with_hook(
            |event| {
                if event == Event::Published(0) {
                    return Err("injected publish failure".into());
                }
                Ok(())
            },
            || f.apply(),
        )
        .unwrap_err();
        assert!(error.contains("回滚"), "{error}");
        f.assert_original();
        let error = with_hook(
            |event| {
                if event == Event::Cleanup(1) {
                    return Err("injected cleanup failure".into());
                }
                Ok(())
            },
            || f.apply(),
        )
        .unwrap_err();
        assert!(error.contains("已提交"), "{error}");
        f.assert_committed();
        f.recover().unwrap();
        f.assert_committed();
    }

    #[test]
    fn recovery_retains_materials_if_external_edit_arrives_after_restore() {
        let f = Fixture::new();
        f.interrupt(Event::BeforeCommit);
        let b = f.spec[1].path.clone();
        let error = with_hook(
            move |event| {
                if event == Event::Restored(0) {
                    fs::write(&b, b"external after restore").unwrap();
                }
                Ok(())
            },
            || f.recover(),
        )
        .unwrap_err();
        assert!(error.contains("冲突"));
        assert_eq!(
            fs::read(&f.spec[1].path).unwrap(),
            b"external after restore"
        );
        let journal: Journal =
            serde_json::from_str(&fs::read_to_string(f.journal()).unwrap()).unwrap();
        assert_eq!(
            fs::read(journal.slots[1].staging.join("original")).unwrap(),
            b"original b"
        );
        assert!(f.recover().unwrap_err().contains("冲突"));
    }

    #[test]
    fn recovery_error_preserves_materials_and_allows_retry() {
        let f = Fixture::new();
        f.interrupt(Event::BeforeCommit);
        let error = with_hook(
            |event| {
                if event == Event::BeforeRestore(1) {
                    return Err("injected recovery IO failure".into());
                }
                Ok(())
            },
            || f.recover(),
        )
        .unwrap_err();
        assert!(error.contains("injected recovery IO failure"));
        assert!(f.journal().exists());
        f.recover().unwrap();
        f.assert_original();
    }

    #[test]
    fn trash_failure_after_deletion_restores_all_slots() {
        let f = Fixture::new();
        let error = apply_snapshot_with(
            f.tmp.path(),
            &f.spec,
            &f.slots(),
            None,
            write_config_file,
            |p| {
                fs::remove_file(p).unwrap();
                Err("injected trash failure after unlink".into())
            },
        )
        .unwrap_err();
        assert!(error.contains("回滚"), "{error}");
        f.assert_original();
        f.recover().unwrap();
    }

    #[test]
    fn rollback_preserves_external_replacement_and_recovery_materials() {
        let f = Fixture::new();
        let a = f.spec[0].path.clone();
        let error = with_hook(
            move |event| {
                if event == Event::Published(0) {
                    fs::write(&a, b"external a").unwrap();
                    return Err("injected later failure".into());
                }
                Ok(())
            },
            || f.apply(),
        )
        .unwrap_err();
        assert!(error.contains("冲突"), "{error}");
        assert_eq!(fs::read(&f.spec[0].path).unwrap(), b"external a");
        assert!(f.recover().unwrap_err().contains("冲突"));
        let journal: Journal =
            serde_json::from_str(&fs::read_to_string(f.journal()).unwrap()).unwrap();
        assert_eq!(
            fs::read(journal.slots[0].staging.join("original")).unwrap(),
            b"original a"
        );
    }

    #[test]
    fn recovering_never_overwrites_external_creation_or_identical_replacement() {
        for (slot, content) in [(0, b"new a".as_slice()), (1, b"external b".as_slice())] {
            let f = Fixture::new();
            f.interrupt(Event::BeforeCommit);
            let target = f.spec[slot].path.clone();
            let error = with_hook(
                move |event| {
                    if event == Event::BeforeRestore(slot) {
                        atomic_write(&target, content).unwrap();
                    }
                    Ok(())
                },
                || f.recover(),
            )
            .unwrap_err();
            assert!(error.contains("冲突"), "{error}");
            assert_eq!(fs::read(&f.spec[slot].path).unwrap(), content);
            assert!(f.journal().exists());
        }
    }

    #[test]
    fn committed_cleanup_never_reverts_later_external_edits() {
        let f = Fixture::new();
        f.interrupt(Event::Cleanup(1));
        fs::write(&f.spec[0].path, b"external after commit").unwrap();
        f.recover().unwrap();
        assert_eq!(fs::read(&f.spec[0].path).unwrap(), b"external after commit");
        assert!(!f.spec[1].path.exists());
        assert!(!f.journal().exists());
    }

    #[test]
    fn committed_cleanup_accepts_an_external_target_symlink() {
        let f = Fixture::new();
        f.interrupt(Event::Cleanup(1));
        let external = f.tmp.path().join("dotfile");
        fs::write(&external, b"external dotfile").unwrap();
        fs::remove_file(&f.spec[0].path).unwrap();
        std::os::unix::fs::symlink(&external, &f.spec[0].path).unwrap();
        f.recover().unwrap();
        assert!(f.spec[0].path.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&external).unwrap(), b"external dotfile");
        assert!(!f.journal().exists());
        f.recover().unwrap();
    }

    #[test]
    fn baseline_and_each_publish_check_detect_external_changes() {
        let f = Fixture::new();
        let baseline = vec![(f.spec[0].path.clone(), Some(b"stale".to_vec()))];
        assert!(apply_snapshot_with(
            f.tmp.path(),
            &f.spec,
            &f.slots(),
            Some(&baseline),
            write_config_file,
            |_| Ok(())
        )
        .unwrap_err()
        .contains("冲突"));
        let c = f.spec[2].path.clone();
        let error = with_hook(
            move |event| {
                if event == Event::BeforePublish(2) {
                    fs::write(&c, b"external creation").unwrap();
                }
                Ok(())
            },
            || f.apply(),
        )
        .unwrap_err();
        assert!(error.contains("冲突"), "{error}");
        assert_eq!(fs::read(&f.spec[0].path).unwrap(), b"original a");
        assert_eq!(fs::read(&f.spec[2].path).unwrap(), b"external creation");
    }

    #[test]
    fn journal_allowlist_rejects_tampering_before_any_recovery_write() {
        for attack in 0..6 {
            let f = Fixture::new();
            f.interrupt(Event::BeforeCommit);
            let victim = f.tmp.path().join("victim");
            fs::write(&victim, b"untouched").unwrap();
            let mut json: Value =
                serde_json::from_str(&fs::read_to_string(f.journal()).unwrap()).unwrap();
            match attack {
                0 => json["slots"][0]["target"] = serde_json::json!(victim),
                1 => json["slots"][0]["staging"] = serde_json::json!(f.tmp.path()),
                2 => {
                    json["slots"][0]["target"] =
                        serde_json::json!(f.tmp.path().join("sub/../a.json"))
                }
                3 => json["id"] = serde_json::json!("../../outside"),
                4 => json["slots"][0]["published"]["sha256"] = serde_json::json!("forged"),
                _ => json["phase"] = serde_json::json!("unknown"),
            }
            atomic_write(&f.journal(), &serde_json::to_vec(&json).unwrap()).unwrap();
            assert!(f.recover().is_err());
            f.assert_committed(); // 拒绝日志不能连带部分回滚其他合法槽位。
            assert_eq!(fs::read(victim).unwrap(), b"untouched");
            assert!(f.journal().exists());
        }
    }

    #[test]
    fn recovery_rejects_symlinked_target_staging_and_journal() {
        for attack in 0..3 {
            let f = Fixture::new();
            f.interrupt(Event::BeforeCommit);
            let victim = f.tmp.path().join("victim");
            fs::write(&victim, b"untouched").unwrap();
            let journal: Journal =
                serde_json::from_str(&fs::read_to_string(f.journal()).unwrap()).unwrap();
            let target = match attack {
                0 => f.spec[0].path.clone(),
                1 => journal.slots[0].staging.clone(),
                _ => f.journal(),
            };
            fs::rename(&target, target.with_extension("saved")).unwrap();
            std::os::unix::fs::symlink(&victim, &target).unwrap();
            assert!(f.recover().is_err());
            assert!(target.symlink_metadata().unwrap().file_type().is_symlink());
            assert_eq!(fs::read(victim).unwrap(), b"untouched");
        }
    }

    #[test]
    fn private_journal_and_staging_preserve_original_mode_and_bytes() {
        let f = Fixture::new();
        fs::set_permissions(&f.spec[0].path, fs::Permissions::from_mode(0o400)).unwrap();
        f.interrupt(Event::BeforeCommit);
        assert_eq!(fs::metadata(f.journal()).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(f.journal().parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        let journal: Journal =
            serde_json::from_str(&fs::read_to_string(f.journal()).unwrap()).unwrap();
        for slot in &journal.slots {
            assert_eq!(fs::metadata(&slot.staging).unwrap().mode() & 0o777, 0o700);
        }
        f.recover().unwrap();
        f.assert_original();
        assert_eq!(fs::metadata(&f.spec[0].path).unwrap().mode() & 0o777, 0o400);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn late_trash_failure_and_restart_preserve_acl_and_xattr() {
        use std::process::Command;
        for restart in [false, true] {
            let f = Fixture::new();
            let a = &f.spec[0].path;
            assert!(Command::new("/usr/bin/xattr")
                .args(["-w", "com.asm.transaction-test", "original attribute"])
                .arg(a)
                .status()
                .unwrap()
                .success());
            assert!(Command::new("/bin/chmod")
                .args([
                    "+a",
                    "everyone allow read,readattr,readextattr,readsecurity"
                ])
                .arg(a)
                .status()
                .unwrap()
                .success());
            let acl = |p: &Path| {
                let out = Command::new("/bin/ls").arg("-le").arg(p).output().unwrap();
                assert!(out.status.success());
                String::from_utf8(out.stdout)
                    .unwrap()
                    .lines()
                    .skip(1)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            let before = acl(a);
            assert!(!before.is_empty());
            if restart {
                f.interrupt(Event::BeforeCommit);
                f.recover().unwrap();
            } else {
                apply_snapshot_with(
                    f.tmp.path(),
                    &f.spec,
                    &f.slots(),
                    None,
                    write_config_file,
                    |_| Err("injected trash failure".into()),
                )
                .unwrap_err();
            }
            f.assert_original();
            assert_eq!(acl(a), before);
            let attribute = Command::new("/usr/bin/xattr")
                .args(["-p", "com.asm.transaction-test"])
                .arg(a)
                .output()
                .unwrap();
            assert!(attribute.status.success());
            assert_eq!(attribute.stdout, b"original attribute\n");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rollback_preserves_external_metadata_edits_too() {
        let f = Fixture::new();
        f.interrupt(Event::BeforeCommit);
        assert!(std::process::Command::new("/usr/bin/xattr")
            .args(["-w", "com.asm.external", "external attribute"])
            .arg(&f.spec[0].path)
            .status()
            .unwrap()
            .success());
        assert!(f.recover().unwrap_err().contains("冲突"));
        assert_eq!(fs::read(&f.spec[0].path).unwrap(), b"new a");
    }

    #[test]
    fn preparation_cleanup_preserves_external_changes_without_blocking_access() {
        for linked in [false, true] {
            let f = Fixture::new();
            let interrupted = catch_unwind(AssertUnwindSafe(|| {
                apply_snapshot_with(
                    f.tmp.path(),
                    &f.spec,
                    &f.slots(),
                    None,
                    |_, _| panic!("process interrupted during preparation"),
                    |_| panic!("preparation must not delete targets"),
                )
            }));
            assert!(interrupted.is_err());
            if linked {
                let external = f.tmp.path().join("dotfile");
                fs::write(&external, b"external content").unwrap();
                fs::remove_file(&f.spec[0].path).unwrap();
                std::os::unix::fs::symlink(external, &f.spec[0].path).unwrap();
            } else {
                fs::write(&f.spec[0].path, b"external content").unwrap();
            }
            f.recover().unwrap();
            assert_eq!(fs::read(&f.spec[0].path).unwrap(), b"external content");
            assert_eq!(f.spec[0].path.symlink_metadata().unwrap().file_type().is_symlink(), linked);
            assert_eq!(fs::read(&f.spec[1].path).unwrap(), b"original b");
            assert!(!f.journal().exists());
            f.recover().unwrap();
        }
    }

    #[test]
    fn preparation_interrupted_inside_atomic_writer_is_recoverable() {
        let f = Fixture::new();
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            apply_snapshot_with(
                f.tmp.path(),
                &f.spec,
                &f.slots(),
                None,
                |path, _| {
                    let pending = path.with_file_name(".publish.tmp-123-456");
                    fs::write(pending, b"partial").unwrap();
                    panic!("process interrupted before atomic rename");
                },
                |_| Ok(()),
            )
        }));
        assert!(interrupted.is_err());
        f.recover().unwrap();
        f.assert_original();
        assert!(!f.journal().exists());
    }
}
