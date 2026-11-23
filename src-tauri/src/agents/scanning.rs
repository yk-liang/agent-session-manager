//! 会话日志的扫描、读取与诊断。

use rusqlite::{Connection, ErrorCode, OpenFlags};
use serde_json::Value;
use std::fmt::Write;
use std::fs;
use std::io::{Error, ErrorKind};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::Duration;

pub(crate) fn sqlite_wal_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push("-wal");
    path.with_file_name(name)
}

fn open_sqlite_snapshot(path: &Path, flags: OpenFlags) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(Duration::from_millis(300))?;
    connection.execute_batch("BEGIN")?;
    // SQLite 延迟到第一次读取才打开 WAL，提前确认可读并固定快照。
    connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
        row.get::<_, i64>(0)
    })?;
    Ok(connection)
}

fn immutable_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            write!(uri, "%{byte:02X}").unwrap();
        }
    }
    uri.push_str("?immutable=1");
    uri
}

fn sqlite_revision(meta: fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}

pub(crate) fn sqlite_snapshot<T>(
    path: &Path,
    load: impl FnOnce(&Connection) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let error = match open_sqlite_snapshot(path, flags) {
        Ok(connection) => return load(&connection),
        Err(error) => error,
    };
    let wal = sqlite_wal_path(path);
    if error.sqlite_error_code() != Some(ErrorCode::CannotOpen) || wal.try_exists()? {
        return Err(error.into());
    }
    // 旧会话可能保留 WAL 模式的库头，但已无 WAL 文件。只在主库静止时读取。
    let before = sqlite_revision(fs::metadata(path)?);
    let uri = immutable_uri(path);
    let connection = open_sqlite_snapshot(Path::new(&uri), flags | OpenFlags::SQLITE_OPEN_URI)?;
    let result = load(&connection)?;
    if wal.try_exists()? || sqlite_revision(fs::metadata(path)?) != before {
        return Err("会话库正在写入，请刷新后重试".into());
    }
    Ok(result)
}

/// 以「非阻塞 + 普通文件复核」方式打开 JSONL 转录，供所有会话解析器共用。
/// 裸 `File::open` 打开 FIFO（read-only）会阻塞到有写端为止——一个被换成
/// FIFO 的会话文件足以卡死整轮扫描以及无关会话的删除。这里用 O_NONBLOCK
/// 打开（对普通文件后续读取无影响），再用 fd 元数据复核确为普通文件，
/// 把 FIFO/目录等特殊文件挡在读取之前（无检查/使用竞态）。
/// 允许读取指向普通文件的符号链接，以兼容用户管理的日志目录。
pub fn open_jsonl(path: &Path) -> std::io::Result<fs::File> {
    open_jsonl_with(path, |_, error| error)
}

pub(crate) fn open_jsonl_diagnosed(path: &Path, operation: &str) -> Result<fs::File, String> {
    open_jsonl_with(path, |stage, error| {
        super::diagnostics::io(operation, stage, path, &error)
    })
}

fn open_jsonl_with<E>(path: &Path, error: impl Fn(&str, Error) -> E) -> Result<fs::File, E> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| error("open", e))?;
    let meta = file.metadata().map_err(|e| error("file_metadata", e))?;
    if !meta.is_file() {
        return Err(error(
            "validate_file_type",
            Error::new(
                ErrorKind::InvalidInput,
                "不是普通文件（FIFO/目录等），拒绝读取",
            ),
        ));
    }
    Ok(file)
}

/// 只将 JSON 语法坏行计为降级；系统读取及 UTF-8 解码失败保留阶段并立即返回。
/// 不把原始行放进诊断，避免复制错误详情时泄露正文或密钥。
pub(crate) fn read_jsonl_checked(
    mut reader: impl std::io::BufRead,
    path: &Path,
    operation: &str,
    mut each: impl FnMut(Value),
) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    let mut line = 0;
    let mut valid = 0;
    let mut bad = 0;
    let mut first_error = None;
    loop {
        bytes.clear();
        line += 1;
        if reader.read_until(b'\n', &mut bytes).map_err(|e| {
            format!(
                "{}\nline: {line}",
                super::diagnostics::io(operation, "read", path, &e)
            )
        })? == 0
        {
            break;
        }
        let text = std::str::from_utf8(&bytes).map_err(|e| {
            super::diagnostics::content(
                operation,
                "decode_utf8",
                path,
                line,
                Some(e.valid_up_to() + 1),
                "InvalidUtf8",
                &e.to_string(),
            )
        })?;
        if text.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(text) {
            Ok(value) => {
                valid += 1;
                each(value);
            }
            Err(error) => {
                bad += 1;
                if first_error.is_none() {
                    first_error = Some(super::diagnostics::content(
                        operation,
                        "parse_json",
                        path,
                        line,
                        Some(error.column()),
                        &format!("Json{:?}", error.classify()),
                        &error.to_string(),
                    ));
                }
            }
        }
    }
    match first_error {
        Some(error) if valid == 0 => Err(error),
        Some(error) => Ok(Some(format!("{}\n{error}", degraded_message(path, bad)))),
        None => Ok(None),
    }
}

/// 解析结果三态：
/// - Ok：完整解析成功
/// - Degraded：解析成功但有瑕疵（如跳过了若干损坏行，统计可能不完整），
///   附带的说明必须进 warnings
/// - Failed：不可读/损坏（附原因，进 warnings）
/// 调用方可依据目录筛选条件跳过成功解析的会话。
pub enum ParseOutcome<T> {
    Ok(T),
    Degraded(T, String),
    Failed(String),
}

impl<T> ParseOutcome<T> {
    /// 拆成 (解析值, 伴随告警)：Failed 时值为 None，告警非空
    pub fn split(self) -> (Option<T>, Vec<String>) {
        match self {
            ParseOutcome::Ok(v) => (Some(v), Vec::new()),
            ParseOutcome::Degraded(v, w) => (Some(v), vec![w]),
            ParseOutcome::Failed(msg) => (None, vec![msg]),
        }
    }
}

/// 逐行遍历 JSONL：空行静默跳过，读取失败或 JSON 解析失败的行计入
/// bad_lines，合法值交给回调处理。各数据源共享的坏行统计口径。
/// 接受 `BufRead::lines()` 产出的行迭代器（调用方可先自行消费首行）。
pub fn for_each_jsonl_line<I, F>(lines: I, mut each: F) -> u32
where
    I: IntoIterator<Item = std::io::Result<String>>,
    F: FnMut(Value),
{
    let mut bad_lines = 0u32;
    for line in lines {
        let Ok(line) = line else {
            bad_lines += 1;
            continue;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            bad_lines += 1;
            continue;
        };
        each(v);
    }
    bad_lines
}

pub fn degraded_message(path: &Path, bad_lines: u32) -> String {
    format!("{:?} 有 {bad_lines} 行无法解析，会话可能不完整", path)
}

/// 返回可读取的目录条目，同时保留目录或条目的读取错误。
pub fn read_dir_warned(dir: &Path, warnings: &mut Vec<String>) -> Vec<fs::DirEntry> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            warnings.push(super::diagnostics::io("catalog", "read_directory", dir, &e));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for entry in rd {
        match entry {
            Ok(e) => out.push(e),
            Err(e) => warnings.push(super::diagnostics::io(
                "catalog",
                "read_directory_entry",
                dir,
                &e,
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor, Read, Write};

    #[test]
    fn checked_reader_preserves_read_failure_after_valid_content() {
        struct BrokenReader;
        impl Read for BrokenReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(Error::from_raw_os_error(libc::EIO))
            }
        }
        let prefix = Cursor::new(b"{\"content\":\"PRIVATE_BODY\"}\n");
        let mut count = 0;
        let error = read_jsonl_checked(
            BufReader::new(prefix.chain(BrokenReader)),
            Path::new("fixture.jsonl"),
            "session_detail",
            |_| count += 1,
        )
        .unwrap_err();
        assert_eq!(count, 1);
        assert!(error.contains("stage: read"));
        assert!(error.contains(&format!("os_code: {}", libc::EIO)));
        assert!(error.contains(&format!("cause: {}", Error::from_raw_os_error(libc::EIO))));
        assert!(error.contains("line: 2"));
        assert!(!error.contains("无法解析"));
        assert!(!error.contains("PRIVATE_BODY"));
    }

    #[test]
    fn checked_reader_distinguishes_utf8_and_json_failures_without_copying_content() {
        let parse = |bytes: &[u8]| {
            read_jsonl_checked(
                Cursor::new(bytes),
                Path::new("fixture.jsonl"),
                "session_detail",
                |_| {},
            )
        };
        let utf8 = parse(b"\n{\"content\":\"PRIVATE_BODY\xff\"}\n").unwrap_err();
        assert!(utf8.contains("stage: decode_utf8"));
        assert!(utf8.contains("error_kind: InvalidUtf8"));
        assert!(utf8.contains("line: 2"));
        assert!(utf8.contains("os_code: none"));
        assert!(!utf8.contains("PRIVATE_BODY"));
        let syntax = parse(b"{\"content\":\"PRIVATE_BODY\",}\n").unwrap_err();
        assert!(syntax.contains("stage: parse_json"));
        assert!(syntax.contains("error_kind: JsonSyntax"));
        assert!(syntax.contains("line: 1"));
        assert!(!syntax.contains("PRIVATE_BODY"));
        let warning = parse(b"{\"ok\":true}\n{\"content\":\"PRIVATE_BODY\",}\n")
            .unwrap()
            .unwrap();
        assert!(warning.contains("有 1 行无法解析"));
        assert!(warning.contains("line: 2"));
        assert!(!warning.contains("PRIVATE_BODY"));
    }

    #[test]
    fn open_jsonl_reads_regular_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "{{\"a\":1}}").unwrap();
        let mut file = open_jsonl(f.path()).unwrap();
        let mut s = String::new();
        file.read_to_string(&mut s).unwrap();
        assert!(s.contains("\"a\":1"));
    }

    #[test]
    fn open_jsonl_missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = open_jsonl(&dir.path().join("nope.jsonl")).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn open_jsonl_refuses_fifo_without_blocking() {
        // 无读写端的 FIFO：裸 File::open(read) 会永久阻塞（卡死扫描）。
        // 独立线程 + 超时确保回归时测试会失败，而不是整套测试永久挂住
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("session.jsonl");
        let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(open_jsonl(&path).map(|_| ()));
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("打开 FIFO 必须在 1 秒内返回");
        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            ErrorKind::InvalidInput,
            "FIFO 必须被拒绝: {err}"
        );
    }

    #[test]
    fn open_jsonl_refuses_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("adir");
        fs::create_dir_all(&sub).unwrap();
        assert!(open_jsonl(&sub).is_err(), "目录不得被当作普通文件读取");
    }
}
