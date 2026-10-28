use super::{
    cache::{stamp, FileCache},
    parse::{normalize, recorded_fields, timestamp},
    types::*,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

pub(super) fn stamps(path: &Path) -> std::io::Result<Vec<super::cache::Stamp>> {
    let wal = path.with_file_name(format!(
        "{}-wal",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let mut stamps = vec![stamp(path)?];
    match stamp(&wal) {
        Ok(value) => stamps.push(value),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(stamps)
}

pub(super) fn read(
    path: &Path,
    old: Option<&FileCache>,
    force: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(FileCache, u64), String> {
    let force = force || old.is_some_and(|old| old.parser_version < 4);
    let stamps = stamps(path).map_err(|e| e.to_string())?;
    if !force && old.is_some_and(|old| old.problem.is_none() && old.stamps == stamps) {
        return Ok((old.unwrap().clone(), 0));
    }
    let conn = crate::agents::opencode::open_db(path)?;
    conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    let has_sessions = conn
        .prepare("SELECT id,time_created FROM session LIMIT 0")
        .is_ok();
    // 只取请求元数据，不读取 part/对话正文。原始 time.created 早于会话创建说明是继承历史，
    // 不能只靠 tokens/时间相等合并，否则会丢掉两次确实发生的相同请求。
    let session_created = if has_sessions {
        "(SELECT time_created FROM session WHERE session.id=message.session_id)"
    } else {
        "NULL"
    };
    let query = format!("SELECT id,session_id,time_created,json_object(
        'modelID',json_extract(data,'$.modelID'),'providerID',json_extract(data,'$.providerID'),
        'completed',json_extract(data,'$.time.completed'),'finish',json_extract(data,'$.finish'),
        'failed',json_extract(data,'$._asm_failed')=1 OR (json_type(data,'$.error') IS NOT NULL AND json_type(data,'$.error') != 'null'),
        'cost',json_extract(data,'$.cost'),'created',json_extract(data,'$.time.created'),'agent',json_extract(data,'$.agent'),
        'mode',json_extract(data,'$.mode'),'path',json_extract(data,'$.path'),
        'tokens',json_extract(data,'$.tokens')), {session_created} FROM message
        WHERE json_valid(data) AND (json_extract(data,'$.role')='assistant'
            OR (json_extract(data,'$.role')='compaction' AND json_type(data,'$.tokens')='object')) ORDER BY session_id,time_created,id");
    let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut result = FileCache {
        parser_version: 4,
        stamps,
        ..Default::default()
    };
    let mut bytes = 0u64;
    for row in rows {
        if cancel() {
            return Err("统计已取消".into());
        }
        let (id, session, time, data, session_created) = row.map_err(|e| e.to_string())?;
        bytes += data.len() as u64;
        let v: Value = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        result.state.saw_assistant = true;
        let Some((tokens, partial)) = normalize(&v["tokens"], "opencode") else {
            result.state.missing_usage += 1;
            continue;
        };
        let model = v["modelID"].as_str().unwrap_or("unknown");
        let model = v["providerID"]
            .as_str()
            .map(|p| format!("{p}/{model}"))
            .unwrap_or_else(|| model.into());
        let created = timestamp(v.get("created"));
        let copy_fingerprint = created.map(|time| {
            format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&(time, &model, &v["agent"], &v["mode"], &v["path"]))
                        .unwrap_or_default()
                )
            )
        });
        result.samples.push(Sample {
            request: super::request_parse::opencode(&v, path),
            key: id,
            session,
            model,
            timestamp: created
                .or_else(|| timestamp(Some(&Value::from(time))))
                .unwrap_or(0),
            tokens,
            details: recorded_fields(&v["tokens"], "opencode"),
            partial,
            offset: 0,
            ordinal: None,
            complete: true,
            replayed: created.zip(session_created).is_some_and(|(t, s)| t < s),
            copy_fingerprint,
            cumulative: false,
            response_id: None,
            last_usage: None,
            thread_total: None,
        });
    }
    let bad: i64 = conn
        .query_row(
            "SELECT count(*) FROM message WHERE NOT json_valid(data)",
            [],
            |r| r.get(0),
        )
        .unwrap_or(1);
    result.state.bad_lines = bad.max(0) as u64;
    Ok((result, bytes))
}

pub(super) fn records(files: &BTreeMap<PathBuf, FileCache>) -> (Vec<Sample>, bool, Vec<String>) {
    let all = super::normalize::unique(files.values().flat_map(|f| f.samples.iter().cloned()));
    let mut groups = HashMap::<String, Vec<Sample>>::new();
    let mut out = Vec::new();
    let mut missing_provenance = false;
    for sample in all {
        if let Some(fingerprint) = &sample.copy_fingerprint {
            groups.entry(fingerprint.clone()).or_default().push(sample);
        } else {
            missing_provenance = true;
            out.push(sample);
        }
    }
    let mut orphaned = false;
    for (fingerprint, samples) in groups {
        let mut originals = Vec::new();
        let mut copies = BTreeMap::<String, Vec<Sample>>::new();
        for sample in samples {
            if sample.replayed {
                copies
                    .entry(sample.session.clone())
                    .or_default()
                    .push(sample);
            } else {
                originals.push(sample);
            }
        }
        // 原始调用即使时间/模型完全相同也各记一次；同一继承链中的历史最多取一份。
        let original_count = originals.len();
        let max_copies = copies.values().map(Vec::len).max().unwrap_or(0);
        if let [original] = originals.as_mut_slice() {
            // 只有一个原始请求时，副本可补齐未采集的流式末帧；仍保留原始身份。
            // 多个真请求碰巧共享元数据时，不能猜测副本对应哪一个。
            let candidates =
                std::iter::once(original.clone()).chain(copies.values().flatten().map(|copy| {
                    let mut copy = copy.clone();
                    copy.key = original.key.clone();
                    copy.session = original.session.clone();
                    copy.replayed = false;
                    copy
                }));
            *original = super::normalize::unique(candidates).pop().unwrap();
        }
        out.extend(originals);
        if max_copies > original_count {
            orphaned |= original_count == 0;
            let mut inherited = copies.into_values().max_by_key(Vec::len).unwrap();
            inherited.sort_by(|a, b| a.key.cmp(&b.key));
            for (index, mut sample) in inherited.into_iter().enumerate().skip(original_count) {
                sample.key = format!("inherited:{fingerprint}:{index}");
                sample.partial |= original_count == 0;
                out.push(sample);
            }
        }
    }
    let partial = missing_provenance || orphaned || out.iter().any(|s| s.partial);
    let mut warnings = Vec::new();
    if orphaned {
        warnings.push("部分 OpenCode 用量只保留了分叉副本，原始请求身份无法完全确认".into());
    }
    if missing_provenance {
        warnings.push("部分 OpenCode 旧计量缺少原始请求元数据，历史分叉去重无法完全确认".into());
    }
    (out, partial, warnings)
}
