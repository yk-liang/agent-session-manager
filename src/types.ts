export interface AgentInfo {
  id: string;
  displayName: string;
  installed: boolean;
  supportsFork: boolean;
  supportsModelNames?: boolean;
  supportsFastCatalog?: boolean;
  dataDir: string | null;
  deletePolicy: "trash" | "permanent";
}

export interface SessionMeta {
  sessionId: string;
  projectDir: string;
  cwd: string | null;
  title: string | null;
  model: string | null;
  /** 数据源确认的模型名称，原始标识仍保留在 model 中。 */
  modelName?: string | null;
  version: string | null;
  gitBranch: string | null;
  entrypoint: string | null;
  firstTs: string | null;
  lastTs: string | null;
  userCount: number;
  assistantCount: number;
  realUserCount: number;
  assistantTextCount: number;
  sizeBytes: number;
  hasTranscript: boolean;
  statsPending?: boolean;
}

export interface SessionDeleteFailure {
  session: SessionMeta;
  error: string;
}

export interface ConversationSearchHit {
  sessionId: string;
  projectDir: string;
  eventIndex: number;
  eventId: string | null;
  role: "user" | "assistant";
  snippet: string;
}

export interface ConversationSearchResult {
  hits: ConversationSearchHit[];
  skipped: number;
  cancelled: boolean;
}

export interface ConversationSearchTarget extends ConversationSearchHit {
  query: string;
}

export interface Project {
  dirName: string;
  displayPath: string;
  sessionCount: number;
  lastActivity: string | null;
}

export interface Catalog {
  projects: Project[];
  sessions: SessionMeta[];
  warnings: string[];
  projectlessSessionIds?: string[];
  sessionGroupHints?: SessionGroupHint[];
}

export interface SessionGroupHint {
  projectDir: string;
  sessionId: string;
  /** 展示项目根目录；null 表示独立自动工作区。源目录身份保持不变。 */
  root: string | null;
}

export interface SessionDetail {
  meta: SessionMeta;
  events: SessionEvent[];
  warnings?: string[];
}

export interface SessionEvent {
  type: "user" | "assistant" | "system" | "context";
  /** 自动注入的运行上下文；保留原文，但不属于用户或助手对话。 */
  context?: { kind: string; label: string; summary?: string | null };
  uuid?: string;
  timestamp?: string;
  isMeta?: boolean;
  subtype?: string;
  content?: unknown;
  message?: {
    role?: string;
    model?: string;
    modelName?: string;
    content: ContentBlock[] | string;
  };
}

export interface ContentBlock {
  /** 数据源保存的工具状态；有部分输出不代表已经完成。 */
  status?: "pending" | "running" | "completed" | "error";
  /** 已确认日志行的引用；展开时向后端取正文。 */
  deferred?: string;
  type: string;
  text?: string;
  thinking?: string;
  name?: string;
  id?: string;
  input?: unknown;
  tool_use_id?: string;
  content?: unknown;
  is_error?: boolean;
}

export interface FilePresence {
  key: string;
  label: string;
  present: boolean;
}

export interface ProfileSummary {
  name: string;
  agent: string;
  createdAt: number;
  files: FilePresence[];
}

export interface CurrentFile {
  key: string;
  label: string;
  path: string;
  present: boolean;
  content: string | null;
}

export interface CurrentConfig {
  agent: string;
  files: CurrentFile[];
}

export interface ConfigValidation {
  valid: boolean;
  error: string | null;
}

export interface WorkspaceFile extends CurrentFile {
  revision: string;
  validation: ConfigValidation;
}

export interface ConfigBackup {
  id: string;
  createdAt: number;
  label: string;
  files: string[];
  legacy: boolean;
}

export interface ConfigWorkspace {
  agent: string;
  revision: string;
  files: WorkspaceFile[];
  profiles: ProfileSummary[];
  matchingProfiles: string[];
  overview: {
    model: string | null;
    provider: string | null;
    endpoint: string | null;
    mcpServers: string[];
  };
  backups: ConfigBackup[];
  warnings: string[];
}

export type ConfigChangeKind = "profile" | "backup";

export interface ConfigFileChange {
  key: string;
  label: string;
  path: string;
  action: "create" | "replace" | "remove" | "unchanged";
  before: string | null;
  after: string | null;
}

export interface ConfigChangePreview {
  kind: ConfigChangeKind;
  target: string;
  title: string;
  currentRevision: string;
  targetRevision: string;
  files: ConfigFileChange[];
}

export interface ConfigActivity {
  dirty: boolean;
  busy: boolean;
}

export interface UsageTokens {
  inputRecorded?: boolean;
  outputRecorded?: boolean;
  inputIncomplete?: boolean;
  outputIncomplete?: boolean;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  reasoningTokens: number;
  unclassifiedTokens: number;
  totalTokens: number;
  cacheReadRecorded?: boolean;
  cacheWriteRecorded?: boolean;
  reasoningRecorded?: boolean;
}
export interface CollectorStatus {
  desired: boolean;
  running: boolean;
  authorization: "enabled" | "disabled" | "requiresApproval" | "unsupported" | "notBundled";
  pid?: number;
  version?: string;
  error?: string;
}
export interface UsageRow extends UsageTokens {
  date: string;
  model: string;
  modelName: string;
  partial: boolean;
}
export interface UsageReport {
  agent: string;
  updatedAt: number;
  rows: UsageRow[];
  hasUsage: boolean;
  partial: boolean;
  fileCount: number;
  measuredFileCount: number;
  warnings: string[];
  scanMs: number;
  bytesRead: number;
}
export interface UsageSummaryRow extends UsageRow { agent: string; bucketStart: number | null }
export interface UsageSummary {
  rows: UsageSummaryRow[];
  health: Record<string, { partial: boolean; warnings: string[] }>;
  revision: string;
}
export interface UsageSummaryQuery { from?: number; to?: number; bounds: number[] }
export interface UsageSync { updatedAt: number; checkedFiles: number; changedFiles: number; bytesRead: number }

export interface RequestMetadata {
  requestId: string | null;
  provider: string | null;
  project: string | null;
  client: string | null;
  kind: string | null;
  effort: string | null;
  tier: string | null;
  version: string | null;
  source: string | null;
  stop: string | null;
  httpStatus: number | null;
  startedAt: number | null;
  finishedAt: number | null;
  timing: "recorded" | "observed" | "estimated" | null;
  latencyMs: number | null;
  ttftMs: number | null;
  ttftScope: "turn" | null;
  scope: "request" | "usage_delta" | "error" | null;
  costUsd: number | null;
}
export interface RequestEvent extends Omit<UsageTokens, "totalTokens"> {
  id: string;
  agent: string;
  time: number;
  session: string;
  model: string;
  modelName: string;
  status: "success" | "failed" | "unknown";
  partial: boolean;
  request: RequestMetadata;
  outputTps: number | null;
}
export interface EventCursor { time: number; id: string; revision: string }
export interface EventQuery {
  agent?: string;
  model?: string;
  status?: string;
  from?: number;
  to?: number;
  cursor?: EventCursor;
  limit?: number;
}
export interface EventPage {
  events: RequestEvent[];
  total: number;
  nextCursor: EventCursor | null;
  revision: string;
  reset: boolean;
  models: string[];
}
