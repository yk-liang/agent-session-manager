use super::{
    cache::FileCache,
    source::sqlite_stamps,
    parse::{normalize, recorded_fields, text, timestamp},
    request_parse::{at, elapsed_ms, string, RequestError, RequestMeta, RequestOutcome},
    types::*,
};
use crate::agents::opencode::{client_id, model_id, open_db, MESSAGE_ERROR_SQL};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

// v0.1.33–v0.1.39 发布的 OpenCode SQLite 解析版本为 4。
pub(super) const PARSER_VERSION: u8 = 5;

struct ForkBoundary {
    parent: String,
    seq: i64,
    inclusive: bool,
}

fn fork_boundaries(conn: &rusqlite::Connection) -> rusqlite::Result<HashMap<String, ForkBoundary>> {
    let Ok(mut stmt) = conn.prepare(
        "WITH forks AS (
            SELECT id, fork_session_id, CASE WHEN json_valid(fork_boundary) THEN fork_boundary END AS boundary
            FROM main.session_v2 WHERE fork_session_id IS NOT NULL
        )
        SELECT f.id, f.fork_session_id, p.seq, json_extract(f.boundary,'$.type')='through'
        FROM forks f JOIN main.session_message p
          ON p.session_id=f.fork_session_id AND p.id=json_extract(f.boundary,'$.messageID')
        WHERE json_extract(f.boundary,'$.type') IN ('before','through')"
    ) else {
        return Ok(HashMap::new());
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get(0)?,
            ForkBoundary {
                parent: row.get(1)?,
                seq: row.get(2)?,
                inclusive: row.get(3)?,
            },
        ))
    })?;
    rows.collect()
}

pub(super) fn read(
    path: &Path,
    old: Option<&FileCache>,
    force: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(FileCache, u64), String> {
    let force = force || old.is_some_and(|old| old.parser_version != PARSER_VERSION);
    let stamps = sqlite_stamps(path).map_err(|e| e.to_string())?;
    if !force && old.is_some_and(|old| old.stamps == stamps) {
        return Ok((old.unwrap().clone(), 0));
    }
    let conn = open_db(path)?;
    conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    let session_fields = conn.prepare("SELECT * FROM session LIMIT 0").ok();
    let session_column = |name: &str| {
        if session_fields
            .as_ref()
            .is_some_and(|stmt| stmt.column_names().contains(&name))
        {
            format!("(SELECT {name} FROM session WHERE session.id=message.session_id)")
        } else {
            "NULL".into()
        }
    };
    // 只取请求元数据，不读取 part/对话正文。原始 time.created 早于会话创建说明是继承历史，
    // 不能只靠 tokens/时间相等合并，否则会丢掉两次确实发生的相同请求。
    let fork_boundaries = fork_boundaries(&conn).map_err(|e| e.to_string())?;
    let seq_column = if conn
        .prepare("SELECT sort_order FROM message LIMIT 0")
        .is_ok()
    {
        "sort_order"
    } else {
        "NULL"
    };
    // 投影保留了 v2 的角色，压缩状态仍在原表。混合库的 v1 行也有 sort_order。
    let completed_compaction = if seq_column == "sort_order" {
        " AND NOT EXISTS (SELECT 1 FROM main.session_message v2
            WHERE v2.id=message.id AND v2.session_id=message.session_id
              AND v2.type='compaction'
              AND CASE WHEN json_valid(v2.data)
                  THEN json_extract(v2.data,'$.status') IS NOT 'completed' ELSE 1 END)"
    } else {
        ""
    };
    let session_created = session_column("time_created");
    let directory = session_column("directory");
    let version = session_column("version");
    let parent_session_id = session_column("parent_id");
    let query = format!("SELECT id,session_id,time_created,json_object(
        'modelID',json_extract(data,'$.modelID'),'providerID',json_extract(data,'$.providerID'),'model',json_extract(data,'$.model'),
        'completed',json_extract(data,'$.time.completed'),'streamed',json_extract(data,'$.time.streamed'),
        'firstToken',json_extract(data,'$.time.firstToken'),'finish',json_extract(data,'$.finish'),
        'directory',{directory},'version',{version},'parentSessionId',{parent_session_id},'role',json_extract(data,'$.role'),
        'variant',json_extract(data,'$.variant'),'tier',coalesce(json_extract(data,'$.serviceTier'),json_extract(data,'$.service_tier'),json_extract(data,'$.tier')),
        'httpStatus',json_extract(data,'$.httpStatus'),'error',{MESSAGE_ERROR_SQL},
        'failed',json_extract(data,'$._asm_failed')=1 OR (json_type(data,'$.error') IS NOT NULL AND json_type(data,'$.error') != 'null'),
        'created',json_extract(data,'$.time.created'),'agent',json_extract(data,'$.agent'),
        'mode',json_extract(data,'$.mode'),'path',json_extract(data,'$.path'),
        'nativeSession',CASE WHEN coalesce(json_extract(data,'$.model.providerID'),json_extract(data,'$.providerID'))='claude-agent'
            THEN coalesce(json_extract(data,'$._asm_native_session'),
               (SELECT json_extract(value,'$.native.sessionID') FROM json_each(data,'$.content')
                WHERE CASE WHEN type='object' THEN length(trim(json_extract(value,'$.native.sessionID')))>0 END LIMIT 1)) END,
        'tokens',json_extract(data,'$.tokens')), {session_created}, {seq_column} FROM message
        WHERE json_valid(data) AND (json_extract(data,'$.role')='assistant'
            OR (json_extract(data,'$.role')='compaction'{completed_compaction})) ORDER BY session_id,time_created,id");
    let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut result = FileCache {
        parser_version: PARSER_VERSION,
        stamps,
        ..Default::default()
    };
    let mut bytes = 0u64;
    let mut positions = HashMap::new();
    let mut parents = Vec::new();
    for row in rows {
        if cancel() {
            return Err("统计已取消".into());
        }
        let (id, session, time, data, session_created, seq) = row.map_err(|e| e.to_string())?;
        bytes += data.len() as u64;
        let v: Value = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        let created = timestamp(v.get("created"));
        let session_created = session_created.and_then(|at| timestamp(Some(&Value::from(at))));
        let parent = fork_boundaries
            .get(&session)
            .zip(seq)
            .filter(|(bound, seq)| {
                *seq >= 0 && (*seq < bound.seq || bound.inclusive && *seq == bound.seq)
            })
            .map(|(bound, seq)| (bound.parent.clone(), seq));
        let replayed = parent.is_some()
            || (seq.is_none() || !fork_boundaries.contains_key(&session))
                && created.zip(session_created).is_some_and(|(t, s)| t < s);
        if let Some(mut sample) = read_message(&v, path, id, session, time) {
            if let Some(seq) = seq.filter(|&seq| seq >= 0) {
                positions.insert((sample.session.clone(), seq), result.samples.len());
            }
            parents.push(parent);
            sample.replayed = replayed;
            result.samples.push(sample);
        }
    }
    let mut copied = Vec::new();
    for (index, parent) in parents.iter().enumerate() {
        let mut target = index;
        let mut visited = std::collections::HashSet::new();
        let mut next = parent.as_ref();
        while let Some(parent) = next.and_then(|key| positions.get(key)).copied() {
            if parent == index || !visited.insert(parent) {
                break;
            }
            target = parent;
            next = parents[parent].as_ref();
        }
        if target != index {
            copied.push((index, target));
        }
    }
    for (index, target) in copied {
        let key = format!("message:{}", result.samples[target].key);
        result.samples[target].copy_fingerprint = Some(key.clone());
        result.samples[index].copy_fingerprint = Some(key);
    }
    Ok((result, bytes))
}

fn request_meta(v: &Value, path: &Path, time: i64) -> RequestMeta {
    let started_at = at(&v["created"]).or_else(|| at(&Value::from(time)));
    let finished_at = ["streamed", "completed"]
        .into_iter()
        .filter_map(|key| at(&v[key]))
        .find(|end| started_at.is_none_or(|start| *end > start));
    let ttft_ms = at(&v["firstToken"])
        .filter(|first| finished_at.is_none_or(|end| *first <= end))
        .zip(started_at)
        .and_then(|(first, start)| elapsed_ms(start, first));
    let provider = string(&v["model"]["providerID"]).or_else(|| string(&v["providerID"]));
    let native_session = (provider.as_deref() == Some("claude-agent"))
        .then(|| text(v.get("nativeSession")).map(str::to_owned))
        .flatten();
    let parent_session_id = string(&v["parentSessionId"]);
    let mut meta = RequestMeta {
        provider,
        native_session,
        project: string(&v["directory"]).or_else(|| string(&v["path"]["cwd"])),
        client: client_id(v),
        kind: Some(
            if v["role"] == "compaction" {
                "compaction"
            } else if parent_session_id.is_some() {
                "subagent"
            } else {
                "main"
            }
            .into(),
        ),
        parent_session_id,
        version: string(&v["version"]),
        effort: string(&v["model"]["variant"]).or_else(|| string(&v["variant"])),
        tier: string(&v["model"]["serviceTier"])
            .or_else(|| string(&v["model"]["service_tier"]))
            .or_else(|| string(&v["model"]["tier"]))
            .or_else(|| string(&v["tier"])),
        http_status: v["httpStatus"]
            .as_u64()
            .or_else(|| v["error"]["status"].as_u64())
            .and_then(|value| u16::try_from(value).ok()),
        error: RequestError::new(v["error"]["kind"].as_str(), v["error"]["message"].as_str()),
        started_at,
        finished_at,
        ttft_ms,
        ttft_scope: ttft_ms.map(|_| "request".into()),
        stop: string(&v["finish"]),
        scope: Some("request".into()),
        source: Some(path.to_string_lossy().into_owned()),
        status: (v["failed"] == true || v["failed"] == 1).then_some(RequestOutcome::Failed),
        ..Default::default()
    };
    meta.duration();
    meta.timing = meta.latency_ms.or(ttft_ms).map(|_| "recorded".into());
    meta
}

fn read_message(v: &Value, path: &Path, id: String, session: String, time: i64) -> Option<Sample> {
    let request = request_meta(v, path, time);
    let measurement = normalize(&v["tokens"], "opencode");
    if measurement.is_none() && !request.failed() {
        return None;
    }
    let model = model_id(v).unwrap_or_else(|| "unknown".into());
    let copy_fingerprint = timestamp(v.get("created")).map(|time| {
        format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(time, &model, &v["agent"], &v["mode"], &v["path"]))
                    .unwrap_or_default()
            )
        )
    });
    let mut sample = Sample::unmeasured(id, &session, model, request.started_at.unwrap_or(0), 0);
    if let Some((tokens, partial)) = measurement {
        sample.tokens = tokens;
        sample.partial = partial;
    }
    sample.details = recorded_fields(&v["tokens"], "opencode");
    sample.request = request;
    sample.complete = true;
    sample.copy_fingerprint = copy_fingerprint;
    Some(sample)
}

pub(super) fn read_legacy(
    path: &Path,
    stamps: Vec<super::source::Stamp>,
) -> Result<(FileCache, u64), String> {
    let mut result = FileCache {
        parser_version: PARSER_VERSION,
        stamps,
        ..Default::default()
    };
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let Ok(mut message) = serde_json::from_slice::<Value>(&data) else {
        return Ok((result, data.len() as u64));
    };
    if message["role"] != "assistant" {
        return Ok((result, data.len() as u64));
    }
    let id = super::parse::text(message.get("id"))
        .or_else(|| path.file_stem().and_then(|id| id.to_str()))
        .unwrap_or_default()
        .to_owned();
    let session = super::parse::text(message.get("sessionID"))
        .or_else(|| {
            path.parent()
                .and_then(Path::file_name)
                .and_then(|id| id.to_str())
        })
        .unwrap_or_default()
        .to_owned();
    result.state.session.clone_from(&session);
    for field in ["created", "streamed", "completed", "firstToken"] {
        message[field] = message["time"][field].take();
    }
    message["tier"] = ["serviceTier", "service_tier", "tier"]
        .into_iter()
        .map(|key| &message[key])
        .find(|field| field.is_string())
        .cloned()
        .unwrap_or(Value::Null);
    if message["error"].is_object() {
        let error = &message["error"];
        message["error"] = serde_json::json!({
            "kind": error["type"].as_str().or_else(|| error["name"].as_str()),
            "message": error["message"].as_str().or_else(|| error["data"]["message"].as_str()),
            "status": error["status"].as_u64().or_else(|| error["data"]["statusCode"].as_u64()),
        });
        message["failed"] = Value::Bool(true);
    }
    message["nativeSession"] = message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|part| super::parse::text(part.pointer("/native/sessionID")))
        .map(|id| Value::String(id.to_owned()))
        .unwrap_or(Value::Null);
    result
        .samples
        .extend(read_message(&message, path, id, session, 0));
    Ok((result, data.len() as u64))
}

pub(super) fn records(files: &BTreeMap<PathBuf, FileCache>) -> Vec<Sample> {
    let all = super::normalize::unique(
        "opencode",
        files.values().flat_map(|f| f.samples.iter().cloned()),
    );
    let mut groups = HashMap::<String, Vec<Sample>>::new();
    let mut out = Vec::new();
    for sample in all {
        if let Some(fingerprint) = &sample.copy_fingerprint {
            groups.entry(fingerprint.clone()).or_default().push(sample);
        } else {
            out.push(sample);
        }
    }
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
            *original = super::normalize::unique("opencode", candidates)
                .pop()
                .unwrap();
        }
        out.extend(originals);
        if max_copies > original_count {
            let mut inherited = copies.into_values().max_by_key(Vec::len).unwrap();
            inherited.sort_by(|a, b| a.key.cmp(&b.key));
            for (index, mut sample) in inherited.into_iter().enumerate().skip(original_count) {
                sample.key = format!("inherited:{fingerprint}:{index}");
                sample.partial |= original_count == 0;
                out.push(sample);
            }
        }
    }
    out
}
