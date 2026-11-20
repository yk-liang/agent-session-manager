use super::{stamp, FileCache};
use crate::{agents::scanning::open_jsonl, usage::parse};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

/// 首尾指纹与偏移用于判断重读是否覆盖旧记录。
pub(in crate::usage) fn covers_observations(current: &FileCache, old: &FileCache) -> bool {
    if !old.stamps.is_empty() && old.stamps == current.stamps {
        return true;
    }
    if current.offset < old.offset || !(64..=2048).contains(&old.head_len) || old.tail.is_empty() {
        return false;
    }
    let Some(path) = current.origin_path.as_deref() else {
        return false;
    };
    let digest = if path.extension().is_some_and(|ext| ext == "zst") {
        archive_fingerprint
    } else {
        fingerprint
    };
    digest(path, 0, old.head_len).ok().as_ref() == Some(&old.head)
        && digest(path, old.offset.saturating_sub(64), old.offset.min(64))
            .ok().as_ref() == Some(&old.tail)
}

// v0.1.33–v0.1.39 发布的 JSONL 解析版本为 3。
pub(super) const PARSER_VERSION: u8 = 4;

fn fingerprint(path: &Path, start: u64, len: u64) -> std::io::Result<String> {
    let mut f = open_jsonl(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut data = Vec::new();
    f.take(len).read_to_end(&mut data)?;
    Ok(format!("{:x}", Sha256::digest(data)))
}

fn archive_fingerprint(path: &Path, start: u64, len: u64) -> std::io::Result<String> {
    let mut stream = zstd::stream::read::Decoder::new(open_jsonl(path)?)?;
    std::io::copy(&mut stream.by_ref().take(start), &mut std::io::sink())?;
    let mut data = Vec::new();
    stream.take(len).read_to_end(&mut data)?;
    Ok(format!("{:x}", Sha256::digest(data)))
}

pub(in crate::usage) fn read(
    path: &Path,
    agent: &str,
    old: Option<&FileCache>,
    force: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(FileCache, u64), String> {
    let force = force || old.is_some_and(|old| old.parser_version != PARSER_VERSION);
    let io_error = |e: std::io::Error| e.to_string();
    let current = stamp(path).map_err(io_error)?;
    if !force && old.is_some_and(|old| old.stamps == [current.clone()]) {
        return Ok((old.unwrap().clone(), 0));
    }
    let compressed = path.extension().is_some_and(|s| s == "zst");
    // 增长且身份及首尾锚点不变才走追加路径；等长重写、替换、截断都重新解析。
    // 手动强制刷新可重建索引，覆盖保留首尾锚点的非常规原地改写。
    let append = !force
        && !compressed
        && old.is_some_and(|old| {
            old.stamps.first().is_some_and(|s| {
                s.inode == current.inode
                    && s.device == current.device
                    && current.len > s.len
                    && old.offset <= current.len
            }) && fingerprint(path, 0, old.head_len).ok().as_ref() == Some(&old.head)
                && fingerprint(path, old.offset.saturating_sub(64), old.offset.min(64))
                    .ok()
                    .as_ref()
                    == Some(&old.tail)
        });
    let mut result = if append {
        old.unwrap().clone()
    } else {
        FileCache::default()
    };
    if result.state.session.is_empty() {
        result.state.session = path.to_string_lossy().into_owned();
    }
    let mut file = open_jsonl(path).map_err(io_error)?;
    let start = if compressed { 0 } else { result.offset };
    file.seek(SeekFrom::Start(start)).map_err(io_error)?;
    let mut reader: Box<dyn BufRead> = if compressed {
        Box::new(BufReader::new(
            zstd::stream::read::Decoder::new(file).map_err(io_error)?,
        ))
    } else {
        Box::new(BufReader::with_capacity(
            64 * 1024,
            file.take(current.len.saturating_sub(start)),
        ))
    };
    let mut position = start;
    let mut bytes_read = 0;
    let mut line = Vec::new();
    result.pending = false;
    result.parser_version = PARSER_VERSION;
    loop {
        if cancel() {
            return Err("统计已取消".into());
        }
        let (bytes, terminated, oversized) =
            bounded_line(reader.as_mut(), &mut line, if agent == "codex" { 32 } else { 16 }).map_err(io_error)?;
        if bytes == 0 {
            break;
        }
        bytes_read += bytes;
        if !terminated {
            result.pending = true;
            break;
        }
        let offset = position;
        position += bytes;
        result.offset = position;
        if oversized || line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // Codex 工具正文占绝大部分字节；无需为与用量无关的巨大对象分配 JSON 树。
        if agent == "codex"
            && ![
                b"\"token_count\"".as_slice(),
                b"\"session_meta\"",
                b"\"turn_context\"",
                b"\"token_usage_record\"",
                b"\"compacted\"",
                b"\"task_started\"",
                b"\"task_complete\"",
                b"\"thread_settings_applied\"",
                b"\"item_completed\"",
                b"_call_output\"",
                b"\"user_message\"",
                b"\"error\"",
            ]
            .iter()
            .any(|needle| memchr::memmem::find(&line, needle).is_some())
        {
            continue;
        }
        if let Ok(v) = serde_json::from_slice(&line) {
            parse::read_line(agent, &v, offset, path, &mut result);
        }
    }
    result.stamps = vec![current];
    if !compressed {
        result.head_len = result.offset.min(2048);
        result.head = fingerprint(path, 0, result.head_len).map_err(io_error)?;
        result.tail = fingerprint(
            path,
            result.offset.saturating_sub(64),
            result.offset.min(64),
        )
        .map_err(io_error)?;
    }
    Ok((result, bytes_read))
}

/// 巨大工具结果只消耗固定上限内存，超过上限仍读到换行以保持后续偏移正确。
fn bounded_line(
    reader: &mut dyn BufRead,
    line: &mut Vec<u8>,
    limit_mib: usize,
) -> std::io::Result<(u64, bool, bool)> {
    line.clear();
    let mut bytes = 0u64;
    let mut large = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok((bytes, false, large));
        }
        let end = memchr::memchr(b'\n', buffer);
        let n = end.map_or(buffer.len(), |i| i + 1);
        if line.len() + n <= limit_mib * 1024 * 1024 && !large {
            line.extend_from_slice(&buffer[..n]);
        } else {
            large = true;
        }
        bytes += n as u64;
        reader.consume(n);
        if end.is_some() {
            return Ok((bytes, true, large));
        }
    }
}
