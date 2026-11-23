//! Codex 活跃与归档日志的目录布局和读取方式。
use crate::agents::{check_catalog_cancelled, read_dir_warned};
use std::fs::{File, Metadata};
use std::io::{self, BufReader, Error, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub(super) const ARCHIVED_SESSIONS: &str = "archived_sessions";

pub(super) fn archived_root(active_root: &Path) -> PathBuf {
    active_root.with_file_name(ARCHIVED_SESSIONS)
}

pub(super) fn is_archived(active_root: &Path, path: &Path) -> bool {
    path.starts_with(archived_root(active_root))
}

pub(super) fn rollouts(
    active_root: &Path,
    include_archived: bool,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    if !active_root.symlink_metadata().is_err_and(|error| error.kind() == ErrorKind::NotFound) {
        for year in read_dir_warned(active_root, warnings) {
            check_catalog_cancelled(cancelled)?;
            for month in read_dir_warned(&year.path(), warnings) {
                for day in read_dir_warned(&month.path(), warnings) {
                    read_rollouts(&day.path(), &mut paths, warnings, cancelled)?;
                }
            }
        }
    }
    if include_archived {
        let directory = archived_root(active_root);
        if !directory
            .symlink_metadata()
            .is_err_and(|error| error.kind() == ErrorKind::NotFound)
        {
            read_rollouts(&directory, &mut paths, warnings, cancelled)?;
        }
    }
    Ok(paths)
}

fn read_rollouts(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    check_catalog_cancelled(cancelled)?;
    for entry in read_dir_warned(directory, warnings) {
        check_catalog_cancelled(cancelled)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("rollout-")
            && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
        {
            let path = entry.path();
            if !path
                .metadata()
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == 0)
            {
                paths.push(path);
            }
        }
    }
    Ok(())
}

pub(super) fn deletion_rollouts(active_root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![active_root.to_path_buf(), archived_root(active_root)];
    let mut paths = Vec::new();
    let mut warnings = Vec::new();
    while let Some(directory) = pending.pop() {
        if !crate::agents::present(&directory)? {
            continue;
        }
        for entry in read_dir_warned(&directory, &mut warnings) {
            let kind = entry.file_type().map_err(|error| format!("无法读取 {:?}：{error}", entry.path()))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() {
                pending.push(entry.path());
            } else if name.starts_with("rollout-") && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst")) {
                paths.push(entry.path());
            }
        }
    }
    if let Some(warning) = warnings.into_iter().next() {
        return Err(warning);
    }
    paths.sort();
    Ok(paths)
}

enum Input {
    Plain(File),
    Zstd(zstd::stream::read::Decoder<'static, BufReader<File>>),
}

pub(super) struct Reader {
    metadata: Metadata,
    input: Input,
}

impl Reader {
    pub fn open(path: &Path) -> io::Result<Self> {
        // 继续由文件描述符验证普通文件，保留 FIFO 非阻塞保护和正常符号链接支持。
        let file = super::super::open_jsonl(path)?;
        let metadata = file.metadata()?;
        let input = if path.extension().is_some_and(|ext| ext == "zst") {
            Input::Zstd(zstd::stream::read::Decoder::new(file)?)
        } else {
            Input::Plain(file)
        };
        Ok(Self { metadata, input })
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// 普通文件可以直接确定正文长度；压缩流需完整读取后才能确认，首屏不为此解压全文件。
    pub fn decoded_size_hint(&self) -> Option<u64> {
        matches!(self.input, Input::Plain(_)).then(|| self.metadata.len())
    }

    /// 跳过当前解压流的前缀；工具引用在新打开的流上调用，不能对压缩文件直接 seek。
    pub fn skip_prefix(&mut self, bytes: u64) -> io::Result<()> {
        match &mut self.input {
            Input::Plain(reader) => {
                let end = reader
                    .stream_position()?
                    .checked_add(bytes)
                    .filter(|end| *end <= self.metadata.len())
                    .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "会话文件已截断"))?;
                reader.seek(SeekFrom::Start(end))?;
            }
            Input::Zstd(reader) => {
                let skipped = io::copy(&mut reader.take(bytes), &mut io::sink())?;
                if skipped != bytes {
                    return Err(Error::new(
                        ErrorKind::UnexpectedEof,
                        "会话解压后的内容已截断",
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.input {
            Input::Plain(reader) => reader.read(buffer),
            Input::Zstd(reader) => reader.read(buffer),
        }
    }
}
