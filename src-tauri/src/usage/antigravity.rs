use super::{
    cache::FileCache,
    request_parse::RequestMeta,
    types::{RecordedFields, RequestIdentities, Sample, Tokens},
};
use crate::agents::antigravity::{
    models::Model,
    proto::Message,
    storage::{self, MODEL_RESPONSE},
};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

mod reconcile;

pub(super) use reconcile::records;
pub(super) const PARSER_VERSION: u8 = 1;

const MODEL_ERROR: i64 = 17;

#[derive(Default)]
struct Usage {
    model: Option<u64>,
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
    reasoning: u64,
    visible: u64,
    keys: Vec<String>,
}

impl Usage {
    fn parse(fields: &Message<'_>) -> Self {
        Self {
            model: fields.number(1).filter(|&id| id != 0),
            input: fields.number(2).unwrap_or_default(),
            output: fields.number(3).unwrap_or_default(),
            cache_write: fields.number(4).unwrap_or_default(),
            cache_read: fields.number(5).unwrap_or_default(),
            reasoning: fields.number(9).unwrap_or_default(),
            visible: fields.number(10).unwrap_or_default(),
            keys: [(7, "message"), (11, "response"), (12, "provider")]
                .into_iter()
                .filter_map(|(number, kind)| fields.text(number).map(|id| format!("{kind}:{id}")))
                .collect(),
        }
    }

    fn tokens(&self) -> Tokens {
        Tokens {
            input_tokens: self
                .input
                .saturating_add(self.cache_read)
                .saturating_add(self.cache_write),
            output_tokens: self.output.max(self.visible.saturating_add(self.reasoning)),
            cache_read_tokens: self.cache_read,
            cache_write_tokens: self.cache_write,
            reasoning_tokens: self.reasoning,
            ..Default::default()
        }
    }

    fn mirrors(&self, primary: &Self) -> bool {
        self.model == primary.model
            && self.tokens() == primary.tokens()
            && self.keys.iter().all(|key| primary.keys.contains(key))
    }
}

struct Attempt {
    usage: Usage,
    failed: bool,
}

fn attempts(fields: &Message<'_>, number: u32, primary: &Usage) -> std::io::Result<Vec<Attempt>> {
    let mut result = Vec::new();
    let mut mirrored = false;
    for bytes in fields.repeated(number) {
        let attempt = Message::decode(bytes)?;
        let usage = Usage::parse(&attempt.message(2)?);
        let failed = attempt.text(3).is_some();
        if !mirrored && usage.tokens().total() > 0 && usage.mirrors(primary) {
            mirrored = true;
            continue;
        }
        result.push(Attempt { usage, failed });
    }
    Ok(result)
}

struct Step {
    kind: i64,
    model: Model,
    time: Option<i64>,
    usage: Usage,
    attempts: Vec<Attempt>,
}

impl Step {
    fn parse(kind: i64, metadata: &[u8]) -> std::io::Result<Self> {
        let fields = Message::decode(metadata)?;
        let usage = Usage::parse(&fields.message(9)?);
        Ok(Self {
            kind,
            model: Model::step(&fields)?,
            time: fields
                .millis(1)?
                .or(fields.millis(8)?)
                .filter(|&time| time > 0),
            attempts: attempts(&fields, 28, &usage)?,
            usage,
        })
    }

    fn status(&self) -> Option<&'static str> {
        match self.kind {
            MODEL_RESPONSE => Some("success"),
            MODEL_ERROR => Some("failed"),
            _ => None,
        }
    }

    fn is_request(&self) -> bool {
        self.status().is_some() || self.usage.tokens().total() > 0
    }
}

struct Generation {
    model: Model,
    usage: Usage,
    attempts: Vec<Attempt>,
    steps: Vec<i64>,
    previous_step: Option<i64>,
    time: Option<i64>,
    ttft: Option<i64>,
    streaming: Option<i64>,
    request_id: Option<String>,
}

impl Generation {
    fn parse(data: &[u8]) -> std::io::Result<Self> {
        let fields = Message::decode(data)?;
        let request = fields.message(1)?;
        let usage = Usage::parse(&request.message(4)?);
        let mut previous_step: Option<i64> = None;
        let mut request_id = None;
        for label in request.repeated(20) {
            let label = Message::decode(label)?;
            match label.text(1) {
                Some("last_step_index") => {
                    previous_step = label.text(2).and_then(|value| value.parse().ok())
                }
                Some("request_id") => request_id = label.text(2).map(str::to_owned),
                _ => {}
            }
        }
        let mut model = Model::generation(&request);
        model.id = model.id.or(usage.model);
        Ok(Self {
            model,
            steps: fields.indices(2)?,
            previous_step,
            time: request.message(9)?.millis(4)?.filter(|&time| time > 0),
            ttft: request.millis(11)?,
            streaming: request.millis(12)?,
            attempts: attempts(&request, 17, &usage)?,
            usage,
            request_id,
        })
    }

    fn step(&self, steps: &BTreeMap<i64, Step>, step_keys: &HashMap<&str, i64>) -> Option<i64> {
        self.usage
            .keys
            .iter()
            .find_map(|id| step_keys.get(id.as_str()).copied())
            .or_else(|| {
                self.steps
                    .iter()
                    .copied()
                    .find(|index| steps.get(index).is_some_and(Step::is_request))
            })
            .or_else(|| {
                self.previous_step
                    .and_then(|index| index.checked_add(1))
                    .filter(|index| steps.get(index).is_some_and(Step::is_request))
            })
            .or_else(|| {
                self.steps
                    .first()
                    .copied()
                    .filter(|_| self.steps.len() == 1)
            })
    }
}

struct Conversation {
    session: String,
    client: Option<String>,
    project: Option<String>,
    created: Option<i64>,
    models: HashMap<u64, Model>,
}

impl Conversation {
    fn add(
        &self,
        file: &mut FileCache,
        key: String,
        step: Option<String>,
        usage: &Usage,
        model: &Model,
        mut request: RequestMeta,
    ) {
        let tokens = usage.tokens();
        let measured = tokens.total() > 0;
        if !measured && request.status.as_deref() != Some("failed") {
            return;
        }
        let (model, effort) = model.resolve(usage.model, &self.models);
        request.client = self.client.clone();
        request.project = self.project.clone();
        request.effort = effort;
        let timestamp = request.started_at.or(self.created).unwrap_or_default();
        file.samples.push(Sample {
            key,
            request,
            session: self.session.clone(),
            model,
            timestamp,
            tokens,
            details: RecordedFields {
                availability_known: true,
                input_recorded: measured,
                output_recorded: measured,
                cache_read_recorded: measured,
                cache_write_recorded: measured,
                reasoning_recorded: measured,
                ..Default::default()
            },
            request_identities: RequestIdentities {
                keys: usage.keys.iter().cloned().collect(),
                steps: step.into_iter().collect(),
            },
            partial: !measured,
            offset: 0,
            ordinal: None,
            complete: true,
            replayed: false,
            copy_fingerprint: None,
            cumulative: false,
            response_id: None,
            last_usage: None,
            thread_total: None,
        });
    }

    fn add_attempts(
        &self,
        file: &mut FileCache,
        key: &str,
        step: Option<&str>,
        attempts: &[Attempt],
        model: &Model,
        time: Option<i64>,
    ) {
        for (ordinal, attempt) in attempts.iter().enumerate() {
            self.add(
                file,
                format!("{key}:attempt:{ordinal}"),
                step.map(|step| format!("{step}:attempt:{ordinal}")),
                &attempt.usage,
                model,
                RequestMeta {
                    started_at: time,
                    status: attempt.failed.then(|| "failed".into()),
                    ..Default::default()
                },
            );
        }
    }
}

pub(super) fn read(
    path: &Path,
    old: Option<&FileCache>,
    force: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(FileCache, u64), String> {
    let stamps = super::cache::sqlite_stamps(path).map_err(|error| error.to_string())?;
    if !force {
        if let Some(old) = old.filter(|old| {
            old.parser_version == PARSER_VERSION && old.problem.is_none() && old.stamps == stamps
        }) {
            return Ok((old.clone(), 0));
        }
    }
    let mut file = FileCache {
        parser_version: PARSER_VERSION,
        stamps,
        ..Default::default()
    };
    let bytes = storage::read(path, |connection, schema| {
        let session = storage::session_id(path)?;
        let (trajectory, mut bytes) = storage::trajectory(connection)?;
        let mut steps = BTreeMap::new();
        let mut generations = Vec::new();
        let mut conversation = Conversation {
            session: session.into(),
            client: path
                .parent()
                .and_then(|parent| parent.parent())
                .and_then(|root| root.file_name())
                .and_then(|name| name.to_str())
                .filter(|name| name.starts_with("antigravity"))
                .map(str::to_owned),
            project: trajectory.cwd,
            created: trajectory.created,
            models: HashMap::new(),
        };
        file.state.session = conversation.session.clone();
        if schema.steps {
            let mut stmt =
                connection.prepare("SELECT idx, step_type, metadata FROM steps ORDER BY idx")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                if cancel() {
                    return Err("统计已取消".into());
                }
                let index: i64 = row.get(0)?;
                let metadata = row.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default();
                bytes += metadata.len() as u64;
                let mut step = Step::parse(row.get(1)?, &metadata)
                    .map_err(|error| format!("步骤 {index}：{error}"))?;
                step.model.id = step.model.id.or(step.usage.model);
                step.model.remember(&mut conversation.models);
                steps.insert(index, step);
            }
        }
        if schema.generations {
            let mut stmt = connection.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                if cancel() {
                    return Err("统计已取消".into());
                }
                let index: i64 = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                bytes += blob.len() as u64;
                let generation = Generation::parse(&blob)
                    .map_err(|error| format!("生成记录 {index}：{error}"))?;
                generation.model.remember(&mut conversation.models);
                generations.push((index, generation));
            }
        }
        let step_keys = steps
            .iter()
            .flat_map(|(&index, step)| step.usage.keys.iter().map(move |key| (key.as_str(), index)))
            .collect();
        for (index, generation) in generations {
            let step_index = generation.step(&steps, &step_keys);
            let step = step_index.and_then(|index| steps.get(&index));
            let key = format!("gen:{session}:{index}");
            let mirror = step_index.map(|index| format!("step:{session}:{index}"));
            let timestamp = generation.time.or(step.and_then(|step| step.time));
            let latency_ms = generation
                .ttft
                .zip(generation.streaming)
                .and_then(|(first, streaming)| first.checked_add(streaming));
            let status = step.and_then(Step::status);
            conversation.add(
                &mut file,
                key.clone(),
                mirror.clone(),
                &generation.usage,
                &generation.model,
                RequestMeta {
                    request_id: generation.request_id,
                    started_at: timestamp,
                    latency_ms,
                    ttft_ms: generation.ttft,
                    timing: latency_ms.or(generation.ttft).map(|_| "recorded".into()),
                    ttft_scope: generation.ttft.map(|_| "request".into()),
                    status: status.map(str::to_owned),
                    ..Default::default()
                },
            );
            conversation.add_attempts(
                &mut file,
                &key,
                mirror.as_deref(),
                &generation.attempts,
                &generation.model,
                timestamp,
            );
        }
        for (index, step) in &steps {
            let key = format!("step:{session}:{index}");
            conversation.add(
                &mut file,
                key.clone(),
                Some(key.clone()),
                &step.usage,
                &step.model,
                RequestMeta {
                    started_at: step.time,
                    status: step.status().map(str::to_owned),
                    ..Default::default()
                },
            );
            conversation.add_attempts(
                &mut file,
                &key,
                Some(&key),
                &step.attempts,
                &step.model,
                step.time,
            );
        }
        Ok(bytes)
    })?;
    Ok((file, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agents, usage};

    const AGENT: &str = "antigravity";

    #[test]
    #[ignore = "需要本机已有 Antigravity 会话"]
    fn real_antigravity_sessions_and_usage() {
        let source = agents::find_source(AGENT).unwrap();
        let catalog = source.catalog(true).unwrap();
        assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
        let mut user_texts = 0;
        let mut assistant_texts = 0;
        let mut tool_calls = 0;
        for session in catalog
            .sessions
            .iter()
            .filter(|session| session.real_user_count > 0)
        {
            let detail = source
                .read_session(&session.project_dir, &session.session_id)
                .unwrap();
            assert!(detail.warnings.is_empty(), "{:?}", detail.warnings);
            assert_eq!(detail.meta.real_user_count, session.real_user_count);
            assert_eq!(
                detail.meta.assistant_text_count,
                session.assistant_text_count
            );
            assert_eq!(source.search_events(session).unwrap(), detail.events);
            user_texts += session.real_user_count;
            assistant_texts += session.assistant_text_count;
            tool_calls += detail
                .events
                .iter()
                .filter_map(|event| event["message"]["content"].as_array())
                .flatten()
                .filter(|block| block["type"] == "tool_use")
                .count();
        }
        assert!(user_texts > 0);
        assert!(assistant_texts > 0);
        let roots = usage::roots(AGENT).unwrap();
        let mut store = usage::Store::open_with_ledger(None, None);
        let first = usage::summary::sync_at(AGENT, &roots, &mut store, &|| false).unwrap();
        let repeated = usage::summary::sync_at(AGENT, &roots, &mut store, &|| false).unwrap();
        assert_eq!(repeated.changed_files, 0);
        usage::event_tests::assert_index_matches_full(&mut store, AGENT);
        let mut tokens = Tokens::default();
        let mut measured = 0;
        let mut failures = 0;
        let files = store.ledger().unwrap().load(AGENT).unwrap();
        for sample in records(&files) {
            failures += usize::from(sample.request.status.as_deref() == Some("failed"));
            if sample.tokens.total() > 0 {
                measured += 1;
                tokens.add(&sample.tokens);
            }
        }
        assert!(measured > 0);
        let summary = usage::summary::read_from(&mut store, Default::default(), &|| false).unwrap();
        assert!(summary.synced_agents.iter().any(|agent| agent == AGENT));
        assert_eq!(
            summary
                .rows
                .iter()
                .map(|row| row.usage.total_tokens)
                .sum::<u64>(),
            tokens.total()
        );
        println!("会话={} 用户消息={user_texts} 助手正文={assistant_texts} 工具调用={tool_calls} 数据库={} 有计量请求={measured} 失败请求={failures} 用量={tokens:?}",
            catalog.sessions.len(), first.changed_files);
    }
}
