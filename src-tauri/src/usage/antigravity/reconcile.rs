//! 步骤、生成记录和重试列表可能保存同一请求的不同副本。
use crate::usage::{
    cache::FileCache,
    types::{Sample, Tokens},
};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

fn merge_tokens(left: &Tokens, right: &Tokens) -> Tokens {
    let uncached = |tokens: &Tokens| {
        tokens
            .input_tokens
            .saturating_sub(tokens.cache_read_tokens)
            .saturating_sub(tokens.cache_write_tokens)
    };
    let cache_read = left.cache_read_tokens.max(right.cache_read_tokens);
    let cache_write = left.cache_write_tokens.max(right.cache_write_tokens);
    let reasoning = left.reasoning_tokens.max(right.reasoning_tokens);
    Tokens {
        input_tokens: uncached(left)
            .max(uncached(right))
            .saturating_add(cache_read)
            .saturating_add(cache_write),
        output_tokens: left.output_tokens.max(right.output_tokens).max(reasoning),
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        reasoning_tokens: reasoning,
        ..Default::default()
    }
}

fn merge(target: &mut Sample, other: Sample) {
    target.tokens = merge_tokens(&target.tokens, &other.tokens);
    if target.model == "unknown"
        || target.model.starts_with("antigravity-model-") && other.model != "unknown"
    {
        target.model = other.model;
    }
    let effort = target
        .request
        .effort
        .clone()
        .or_else(|| other.request.effort.clone());
    target.request.merge(&other.request);
    target.request.effort = effort;
    target.timestamp = target
        .request
        .started_at
        .unwrap_or_else(|| target.timestamp.max(other.timestamp));
    target.request_identities.merge(&other.request_identities);
    target.details.refine(&other.details);
    target.partial &= other.partial;
}

pub(in crate::usage) fn records(files: &BTreeMap<PathBuf, FileCache>) -> Vec<Sample> {
    let mut samples: Vec<_> = files
        .values()
        .flat_map(|file| file.samples.iter().cloned())
        .collect();
    samples.sort_by(|a, b| a.key.cmp(&b.key));
    let mut groups = Vec::<Option<Sample>>::new();
    let mut identities = HashMap::<String, usize>::new();
    let (keyed, unkeyed): (Vec<_>, Vec<_>) = samples
        .into_iter()
        .partition(|sample| !sample.request_identities.keys.is_empty());
    for sample in keyed {
        let mut matches: Vec<_> = sample
            .request_identities
            .keys
            .iter()
            .filter_map(|key| identities.get(key).copied())
            .collect();
        matches.sort_unstable();
        matches.dedup();
        let index = if let Some((&first, rest)) = matches.split_first() {
            for &index in rest {
                let other = groups[index].take().unwrap();
                merge(groups[first].as_mut().unwrap(), other);
            }
            merge(groups[first].as_mut().unwrap(), sample);
            first
        } else {
            groups.push(Some(sample));
            groups.len() - 1
        };
        for key in &groups[index].as_ref().unwrap().request_identities.keys {
            identities.insert(key.clone(), index);
        }
    }
    let mut step_owners = HashMap::<String, Option<usize>>::new();
    for (index, sample) in groups
        .iter()
        .enumerate()
        .filter_map(|(i, group)| group.as_ref().map(|s| (i, s)))
    {
        for step in &sample.request_identities.steps {
            step_owners
                .entry(step.clone())
                .and_modify(|owner| *owner = None)
                .or_insert(Some(index));
        }
    }
    let mut fallbacks = HashMap::<String, usize>::new();
    for sample in unkeyed {
        let key = sample
            .request_identities
            .steps
            .first()
            .unwrap_or(&sample.key)
            .clone();
        // 同一个步骤可能包含多个有明确身份的尝试；只有唯一匹配时才合并无身份副本。
        if let Some(index) = step_owners
            .get(&key)
            .copied()
            .flatten()
            .or_else(|| fallbacks.get(&key).copied())
        {
            merge(groups[index].as_mut().unwrap(), sample);
        } else {
            fallbacks.insert(key, groups.len());
            groups.push(Some(sample));
        }
    }
    groups
        .into_iter()
        .flatten()
        .map(|mut sample| {
            if let Some(key) = sample
                .request_identities
                .keys
                .first()
                .or(sample.request_identities.steps.first())
            {
                sample.key = key.clone();
            }
            sample
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{
        ledger,
        request_parse::RequestMeta,
        types::{RecordedFields, RequestIdentities},
    };

    #[test]
    fn reconciles_late_identities_without_merging_distinct_attempts() {
        let sample = |key: &str, step: &str, aliases: &[&str], tokens: u64| Sample {
            key: key.into(),
            model: "gemini-4-hydrogen".into(),
            session: "session".into(),
            request: RequestMeta::default(),
            request_identities: RequestIdentities {
                keys: aliases.iter().map(|key| (*key).into()).collect(),
                steps: [step.into()].into(),
            },
            timestamp: 1,
            tokens: Tokens {
                input_tokens: tokens,
                ..Default::default()
            },
            details: RecordedFields::default(),
            partial: false,
            offset: 0,
            ordinal: None,
            complete: true,
            replayed: false,
            claude_fallback: false,
            copy_fingerprint: None,
            cumulative: false,
            response_id: None,
            last_usage: None,
            thread_total: None,
        };
        let old = FileCache {
            samples: vec![sample("gen:session:1", "step:session:1", &[], 5)],
            ..Default::default()
        };
        let incoming = FileCache {
            samples: vec![
                sample("gen:session:1", "step:session:1", &["message:first"], 6),
                sample("gen:session:2", "step:session:1", &["message:second"], 7),
                sample("gen:session:3", "step:copy:1", &["response:first"], 6),
                sample(
                    "step:copy:1",
                    "step:copy:1",
                    &["message:first", "response:first"],
                    6,
                ),
                sample("step:previous:1", "step:previous:1", &[], 6),
                sample("step:previous:2", "step:previous:1", &["message:first"], 6),
            ],
            ..Default::default()
        };
        let combined = ledger::sources::merge("antigravity", &old, &incoming);
        let records = records(&BTreeMap::from([("session.db".into(), combined)]));
        assert_eq!(records.len(), 2);
        assert_eq!(
            records
                .iter()
                .map(|sample| sample.tokens.total())
                .sum::<u64>(),
            13
        );
    }
}
