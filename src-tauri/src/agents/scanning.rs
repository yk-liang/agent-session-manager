//! 扫描基础设施：解析结果三态化与 warning-aware 目录遍历。

use serde_json::Value;
use std::fs;
use std::io::{Error, ErrorKind};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// 以「非阻塞 + 普通文件复核」方式打开 JSONL 转录，供所有会话解析器共用。
/// 裸 `File::open` 打开 FIFO（read-only）会阻塞到有写端为止——一个被换成
/// FIFO 的会话文件足以卡死整轮扫描以及无关会话的删除。这里用 O_NONBLOCK
/// 打开（对普通文件后续读取无影响），再用 fd 元数据复核确为普通文件，
/// 把 FIFO/目录等特殊文件挡在读取之前（无检查/使用竞态）。
/// 不加 O_NOFOLLOW：agent 的会话文件可能是 dotfiles 管理的符号链接，
/// 跟随到普通文件读取是正常预期。
pub fn open_jsonl(path: &Path) -> std::io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "不是普通文件（FIFO/目录等），拒绝读取",
        ));
    }
    Ok(file)
}

/// 解析结果三态：
/// - Ok：完整解析成功
/// - Degraded：解析成功但有瑕疵（如跳过了若干损坏行，统计可能不完整），
///   附带的说明必须进 warnings
/// - Failed：不可读/损坏（附原因，进 warnings）
/// "与本应用无关"（如 Codex 非交互会话）由调用方依据解析结果判断后
/// 静默跳过，不算错误、不进 warnings。
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

/// Degraded 解析态的统一措辞：有坏行被跳过时提醒会话信息可能不完整
pub fn degraded_message(path: &Path, bad_lines: u32) -> String {
    format!("{:?} 有 {bad_lines} 行无法解析，会话信息可能不完整", path)
}

/// warning-aware 目录读取：目录本身或个别条目读取失败都记入 warnings，
/// 只返回成功读到的条目，替代会静默丢弃错误的 read_dir().flatten()。
pub fn read_dir_warned(dir: &Path, warnings: &mut Vec<String>) -> Vec<fs::DirEntry> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            warnings.push(format!("无法读取 {:?}: {e}", dir));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for entry in rd {
        match entry {
            Ok(e) => out.push(e),
            Err(e) => warnings.push(format!("读取 {:?} 目录项失败: {e}", dir)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

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
        assert_eq!(err.kind(), ErrorKind::InvalidInput, "FIFO 必须被拒绝: {err}");
    }

    #[test]
    fn open_jsonl_refuses_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("adir");
        fs::create_dir_all(&sub).unwrap();
        assert!(open_jsonl(&sub).is_err(), "目录不得被当作普通文件读取");
    }
}
