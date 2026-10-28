use serde::{Deserialize, Serialize};

/// 输入已包含缓存，输出已包含思考；细分字段不再次计入总量。
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub unclassified_tokens: u64,
}
impl Tokens {
    pub fn total(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.unclassified_tokens)
    }
    pub fn add(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.unclassified_tokens = self
            .unclassified_tokens
            .saturating_add(other.unclassified_tokens);
    }
}

/// 字段未提供与明确记录为 0 不同；incomplete 表示汇总包含缺少该分项的调用。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedFields {
    #[serde(default)]
    pub input_recorded: bool,
    #[serde(default)]
    pub output_recorded: bool,
    #[serde(default)]
    pub input_incomplete: bool,
    #[serde(default)]
    pub output_incomplete: bool,
    /// 旧账本没有分项可用性，读取时只能保守迁移，不能把未知的零认作实测。
    #[serde(default)]
    pub availability_known: bool,
    pub cache_read_recorded: bool,
    pub cache_write_recorded: bool,
    pub reasoning_recorded: bool,
}
impl RecordedFields {
    pub fn merge(&mut self, other: &Self) {
        self.input_recorded |= other.input_recorded;
        self.output_recorded |= other.output_recorded;
        self.input_incomplete |= other.input_incomplete;
        self.output_incomplete |= other.output_incomplete;
        self.availability_known |= other.availability_known;
        self.cache_read_recorded |= other.cache_read_recorded;
        self.cache_write_recorded |= other.cache_write_recorded;
        self.reasoning_recorded |= other.reasoning_recorded;
    }
    pub fn refine(&mut self, other: &Self) {
        let input_incomplete = self.input_incomplete && other.input_incomplete;
        let output_incomplete = self.output_incomplete && other.output_incomplete;
        self.merge(other);
        self.input_incomplete = input_incomplete;
        self.output_incomplete = output_incomplete;
    }
}

/// 只缓存计量元数据，不保存用户输入、工具输出或凭据。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Sample {
    #[serde(default)]
    pub request: super::request_parse::RequestMeta,
    pub key: String,
    pub session: String,
    pub model: String,
    /// 计量事件的 Unix 毫秒；0 表示缺失，不使用文件 mtime 补齐。
    pub timestamp: i64,
    pub tokens: Tokens,
    #[serde(default)]
    pub details: RecordedFields,
    pub partial: bool,
    /// 完整 JSONL 行在未压缩数据中的起始字节位置。
    pub offset: u64,
    pub ordinal: Option<u64>,
    pub complete: bool,
    /// Claude sidechain 可能重放主请求，优先保留原始主记录。
    #[serde(default)]
    pub replayed: bool,
    /// OpenCode fork 会改写消息 ID，保留只含原始请求元数据的摘要以识别复制历史。
    #[serde(default)]
    pub copy_fingerprint: Option<String>,
    /// Codex 保留累计快照，跨分页建立基线后才转为一次调用的增量。
    pub cumulative: bool,
    #[serde(default)]
    pub response_id: Option<String>,
    #[serde(default)]
    pub last_usage: Option<Tokens>,
    #[serde(default)]
    pub thread_total: Option<Tokens>,
}
impl Sample {
    pub fn migrate_availability(&mut self) {
        if !self.details.availability_known {
            self.details.input_recorded = self.tokens.input_tokens > 0 || !self.partial;
            self.details.output_recorded = self.tokens.output_tokens > 0 || !self.partial;
            self.details.input_incomplete = self.partial;
            self.details.output_incomplete = self.partial;
            self.details.availability_known = true;
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct ParserState {
    #[serde(default)]
    pub requests: super::request_parse::RequestState,
    pub session: String,
    pub model: String,
    pub parent_session: Option<String>,
    pub history_base: Option<HistoryBase>,
    /// fork 边界属于父线程，不能用于过滤子线程自己的 ordinal。
    pub parent_before: Option<u64>,
    pub created_at: Option<i64>,
    pub bad_lines: u64,
    pub missing_usage: u64,
    pub saw_assistant: bool,
    /// 缺计量的分块可能稍后补齐；不把每个分块当成一次漏计请求。
    pub unmeasured_ids: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub compactions: std::collections::BTreeSet<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct HistoryBase {
    pub thread_id: String,
    pub end_byte_offset: Option<u64>,
    pub end_ordinal_exclusive: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    pub date: String,
    pub model: String,
    pub model_name: String,
    #[serde(flatten)]
    pub tokens: Tokens,
    #[serde(flatten)]
    pub details: RecordedFields,
    pub total_tokens: u64,
    pub partial: bool,
}
#[cfg(test)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub agent: String,
    pub updated_at: i64,
    pub rows: Vec<UsageRow>,
    pub has_usage: bool,
    pub partial: bool,
    pub file_count: usize,
    pub measured_file_count: usize,
    pub warnings: Vec<String>,
    pub scan_ms: f64,
    pub bytes_read: u64,
}
