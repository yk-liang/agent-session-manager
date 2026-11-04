//! 读取失败的本地诊断：从出错进程采集，不读取环境变量、配置或会话正文。
use std::{io, path::Path};

fn context(
    operation: &str,
    stage: &str,
    path: &Path,
    kind: &str,
    code: Option<i32>,
    cause: &str,
) -> String {
    let executable = std::env::current_exe().ok();
    let collector = std::env::args_os().any(|arg| arg == "--usage-collector");
    let process = if collector {
        "usage_collector"
    } else if executable
        .as_ref()
        .is_some_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
    {
        "gui"
    } else {
        "test_or_cli"
    };
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let limits = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
        format!(
            "nofile_soft: {}\nnofile_hard: {}",
            limit.rlim_cur, limit.rlim_max
        )
    } else {
        "nofile_soft: unavailable\nnofile_hard: unavailable".into()
    };
    format!(
        "operation: {operation}\n\
        stage: {stage}\n\
        error_kind: {kind}\n\
        os_code: {}\n\
        cause: {cause}\n\
        path: {}\n\
        occurred_at: {}\n\
        app_version: {}\n\
        platform: {}\n\
        architecture: {}\n\
        process: {process}\n\
        pid: {}\n\
        uid: {}\n\
        euid: {}\n\
        executable: {}\n\
        {limits}",
        code.map_or_else(|| "none".into(), |code| code.to_string()),
        path.display(),
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        env!("CARGO_PKG_VERSION"),
        platform(),
        std::env::consts::ARCH,
        std::process::id(),
        unsafe { libc::getuid() },
        unsafe { libc::geteuid() },
        executable
            .as_deref()
            .map_or_else(|| "unavailable".into(), |p| p.display().to_string())
    )
}

fn platform() -> String {
    #[cfg(target_os = "macos")]
    {
        // sysctl 不需要打开文件或启动子进程；即使 EMFILE，也能记录 GUI 的系统版本。
        let mut value = [0u8; 64];
        let mut size = value.len();
        if unsafe {
            libc::sysctlbyname(
                c"kern.osproductversion".as_ptr(),
                value.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } == 0
            && size <= value.len()
        {
            return format!(
                "macOS {}",
                String::from_utf8_lossy(&value[..size]).trim_end_matches('\0')
            );
        }
    }
    std::env::consts::OS.into()
}

pub fn io(operation: &str, stage: &str, path: &Path, error: &io::Error) -> String {
    let subject = if operation.starts_with("usage_") {
        "用量数据"
    } else {
        "会话文件"
    };
    format!(
        "{subject}访问失败：{}\n{}",
        path.display(),
        context(
            operation,
            stage,
            path,
            &format!("{:?}", error.kind()),
            error.raw_os_error(),
            &error.to_string()
        )
    )
}

pub fn database(operation: &str, stage: &str, path: &Path, error: &rusqlite::Error) -> String {
    let kind = error
        .sqlite_error()
        .map_or_else(|| "SQLiteError".into(), |e| format!("SQLite{:?}", e.code));
    // SQLite 的扩展码不是系统 errno；没有原始 OS 错误时不可猜测成权限错误。
    let code = error
        .sqlite_error()
        .map_or_else(|| "none".into(), |e| e.extended_code.to_string());
    format!(
        "用量数据库操作失败：{}\n{}\nsqlite_code: {code}",
        path.display(),
        context(operation, stage, path, &kind, None, &error.to_string())
    )
}

pub fn content(
    operation: &str,
    stage: &str,
    path: &Path,
    line: usize,
    column: Option<usize>,
    kind: &str,
    cause: &str,
) -> String {
    format!(
        "会话内容无法解析：{}（第 {line} 行）\n{}\nline: {line}\ncolumn: {}",
        path.display(),
        context(operation, stage, path, kind, None, cause),
        column.map_or_else(|| "none".into(), |n| n.to_string())
    )
}
