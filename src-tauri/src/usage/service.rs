//! macOS 登录后台服务。沿用用户已有的授权与数据目录。
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
};

pub(super) const PLIST: &str = "com.liangyukun.agent-session-manager.usage-collector.plist";
pub(super) fn directory() -> Result<PathBuf, String> {
    Ok(dirs::data_dir()
        .ok_or("无法定位应用数据目录")?
        .join("Agent Session Manager/collector"))
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Settings {
    pub enabled: bool,
    /// 登录服务没有启动终端的环境变量，保存前台实际解析的数据根，避免采集另一份目录。
    pub roots: BTreeMap<String, Vec<PathBuf>>,
}
impl Settings {
    pub fn load(dir: &Path) -> Result<Self, String> {
        serde_json::from_slice(
            &fs::read(dir.join("settings.json")).map_err(|_| "无法读取后台采集设置")?,
        )
        .map_err(|_| "后台采集设置损坏，已保留原文件".into())
    }
}

pub(super) fn prepare_directory(dir: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|_| "无法创建采集服务目录")?;
    if fs::symlink_metadata(dir)
        .map_err(|_| "无法读取采集服务目录")?
        .file_type()
        .is_symlink()
    {
        return Err("采集服务目录不能是符号链接".into());
    }
    Ok(())
}
pub(super) fn write_json(dir: &Path, name: &str, value: &impl Serialize) -> Result<(), String> {
    prepare_directory(dir)?;
    let tmp = dir.join(format!(".{name}-{}.tmp", std::process::id()));
    let write = || -> Result<(), String> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)
            .map_err(|_| "无法写入采集服务状态")?;
        file.write_all(&serde_json::to_vec(value).map_err(|_| "无法编码采集服务状态")?)
            .map_err(|_| "无法写入采集服务状态")?;
        file.sync_all().map_err(|_| "采集服务状态未能落盘")?;
        fs::rename(&tmp, dir.join(name)).map_err(|_| "无法保存采集服务状态")?;
        File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|_| "采集服务状态未能落盘".into())
    };
    let result = write();
    let _ = fs::remove_file(tmp);
    result
}

/// flock 的生命周期等于进程；即使崩溃留下 PID 文件，也不会误报仍在运行。
pub(super) struct ProcessLock(File);
impl ProcessLock {
    pub fn acquire(dir: &Path) -> Result<Option<Self>, String> {
        prepare_directory(dir)?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(dir.join("process.lock"))
            .map_err(|_| "无法锁定后台采集进程")?;
        if !file.metadata().map_err(|_| "无法读取采集锁")?.is_file() {
            return Err("采集锁必须是普通文件".into());
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err("无法锁定后台采集进程".into())
            };
        }
        file.set_len(0)
            .and_then(|_| write!(file, "{}", std::process::id()))
            .map_err(|_| "无法保存采集进程身份")?;
        Ok(Some(Self(file)))
    }
}
impl Drop for ProcessLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn running(dir: &Path) -> bool {
    let Ok(file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join("process.lock"))
    else {
        return false;
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return false;
    }
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0;
    locked && std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RuntimeStatus {
    pub pid: u32,
    pub version: String,
    pub collected_at: BTreeMap<String, i64>,
    pub scan_ms: f64,
    pub bytes_read: u64,
}
pub fn ensure_login_service() -> Result<(), String> {
    let authorization = platform::status();
    if matches!(authorization, "unsupported" | "notBundled") {
        return Ok(());
    }
    let dir = directory()?;
    let old = match fs::symlink_metadata(dir.join("settings.json")) {
        Ok(_) => Some(Settings::load(&dir)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("无法检查后台采集设置：{error}")),
    };
    let settings = Settings {
        // 本地 ad hoc 签名的服务可能被 launchd 以签名约束终止，并被 KeepAlive 反复拉起。
        // 升级时撤销这类旧注册；只有带团队签名的构建才提供登录后台服务。
        enabled: old.as_ref().is_some_and(|settings| settings.enabled) && platform::can_register(),
        roots: super::AGENTS
            .into_iter()
            .map(|id| super::roots(id).map(|roots| (id.into(), roots)))
            .collect::<Result<_, _>>()?,
    };
    if old.as_ref() != Some(&settings) {
        write_json(&dir, "settings.json", &settings)?;
    }
    if settings.enabled {
        let previous: Option<RuntimeStatus> = fs::read(dir.join("status.json"))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok());
        let upgrade = authorization == "enabled"
            && running(&dir)
            && previous.is_some_and(|status| status.version != env!("CARGO_PKG_VERSION"));
        if upgrade {
            platform::set_enabled(false)?;
            // unregister 返回时进程可能还在退出，给 SQLite 完成当前事务的时间。
            for _ in 0..30 {
                if !running(&dir) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        if authorization == "disabled" || upgrade {
            platform::set_enabled(true)?;
        }
    } else if authorization != "disabled" {
        platform::set_enabled(false)?;
    }
    Ok(())
}

pub fn check_login_service() {
    if let Ok(dir) = directory() {
        if Settings::load(&dir).is_ok_and(|settings| settings.enabled) {
            let _ = write_json(&dir, "check.json", &chrono::Utc::now().timestamp_millis());
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::{
        msg_send,
        rc::{autoreleasepool, Retained},
        runtime::{AnyClass, AnyObject, Bool},
    };
    use objc2_foundation::{NSError, NSString};
    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {
        static SMAppServiceErrorDomain: *const AnyObject;
    }
    fn service() -> Option<Retained<AnyObject>> {
        // 保证框架被加载，运行时检查使旧版 macOS 不会调用不存在的类。
        unsafe {
            std::ptr::read_volatile(&raw const SMAppServiceErrorDomain);
        }
        let class = AnyClass::get(c"SMAppService")?;
        Some(unsafe {
            msg_send![class, agentServiceWithPlistName: &*NSString::from_str(super::PLIST)]
        })
    }
    fn bundled() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|p| {
                p.parent()?
                    .parent()
                    .map(|p| p.join("Library/LaunchAgents").join(super::PLIST))
            })
            .is_some_and(|p| p.is_file())
    }
    pub fn can_register() -> bool {
        static SIGNED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *SIGNED.get_or_init(|| {
            let Ok(executable) = std::env::current_exe() else {
                return false;
            };
            let Ok(result) = std::process::Command::new("/usr/bin/codesign")
                .args(["--display", "--verbose=4"])
                .arg(executable)
                .output()
            else {
                return false;
            };
            result.status.success() && super::team_signed(&String::from_utf8_lossy(&result.stderr))
        })
    }
    pub fn status() -> &'static str {
        if !bundled() {
            return "notBundled";
        }
        autoreleasepool(|_| {
            let Some(service) = service() else {
                return "unsupported";
            };
            let status: isize = unsafe { msg_send![&*service, status] };
            match status {
                1 => "enabled",
                2 => "requiresApproval",
                _ => "disabled",
            }
        })
    }
    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        autoreleasepool(|_| {
            let service = service().ok_or("后台采集需要 macOS 13 或更新版本")?;
            let mut error: *mut NSError = std::ptr::null_mut();
            let ok: Bool = unsafe {
                if enabled {
                    msg_send![&*service, registerAndReturnError: &mut error]
                } else {
                    msg_send![&*service, unregisterAndReturnError: &mut error]
                }
            };
            if ok.as_bool() || enabled && status() == "requiresApproval" {
                Ok(())
            } else {
                Err(format!(
                    "后台采集注册操作失败：{}",
                    unsafe { error.as_ref() }
                        .map(|e| e.localizedDescription().to_string())
                        .unwrap_or_else(|| "系统未提供错误信息".into())
                ))
            }
        })
    }
}
#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn can_register() -> bool {
        false
    }
    pub fn status() -> &'static str {
        "unsupported"
    }
    pub fn set_enabled(_: bool) -> Result<(), String> {
        Err("当前系统不支持后台采集服务".into())
    }
}

#[cfg(any(target_os = "macos", test))]
fn team_signed(metadata: &str) -> bool {
    metadata.lines().any(|line| {
        line.strip_prefix("TeamIdentifier=")
            .is_some_and(|team| !team.trim().is_empty() && team != "not set")
    }) && metadata.lines().any(|line| line.starts_with("Authority="))
        && !metadata.lines().any(|line| line == "Signature=adhoc")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_registration_requires_team_signing() {
        assert!(!team_signed("Signature=adhoc\nTeamIdentifier=not set\n"));
        assert!(!team_signed("TeamIdentifier=ABCDE12345\n"));
        assert!(!team_signed(""));
        assert!(team_signed(
            "Authority=Developer ID Application: Example\nTeamIdentifier=ABCDE12345\n"
        ));
        assert!(team_signed(
            "Authority=Apple Development: Example\nTeamIdentifier=ABCDE12345\n"
        ));
    }
    #[test]
    fn process_lock_prevents_two_collectors_and_stale_status_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let first = ProcessLock::acquire(dir.path()).unwrap().unwrap();
        assert!(running(dir.path()));
        assert!(ProcessLock::acquire(dir.path()).unwrap().is_none());
        drop(first);
        assert!(!running(dir.path()));
        assert!(ProcessLock::acquire(dir.path()).unwrap().is_some());
    }
    #[test]
    fn settings_are_durable_private_and_corruption_is_not_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            enabled: false,
            roots: BTreeMap::new(),
        };
        write_json(dir.path(), "settings.json", &settings).unwrap();
        assert!(!Settings::load(dir.path()).unwrap().enabled);
        assert_eq!(
            fs::metadata(dir.path().join("settings.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::write(dir.path().join("settings.json"), "invalid").unwrap();
        assert!(Settings::load(dir.path()).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("settings.json")).unwrap(),
            "invalid"
        );
    }
}
