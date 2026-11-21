export interface AgentInfo {
  id: string;
  displayName: string;
  installed: boolean;
  supportsFork: boolean;
  supportsArchive: boolean;
  supportsArchivedSessions: boolean;
  resumeEntrypoint?: string;
  dataDir: string | null;
  deletePolicy: "trash" | "permanent" | "readOnly";
}

export interface SessionMeta {
  sessionId: string;
  projectDir: string;
  cwd: string | null;
  title: string | null;
  model: string | null;
  version: string | null;
  client?: string | null;
  gitBranch: string | null;
  entrypoint: string | null;
  firstTs: string | null;
  lastTs: string | null;
  userCount: number;
  assistantCount: number;
  realUserCount: number;
  assistantTextCount: number;
  sizeBytes?: number;
  hasTranscript: boolean;
  archived?: boolean;
  statsPending?: boolean;
  /** 仅前端：统计仍在后台计算，消息数沿用同一会话上次确认的值；未知统计不会带此标记。 */
  statsRetained?: boolean;
  /** 本次应用运行中实际打开正文失败的诊断；摘要刷新不能视为正文恢复。 */
  readError?: string;
}

export interface CatalogFilter {
  includeEmpty: boolean;
  includeArchived: boolean;
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
  revision?: number;
  projects: Project[];
  sessions: SessionMeta[];
  warnings: string[];
  projectlessSessionIds?: string[];
  sessionGroupHints?: SessionGroupHint[];
  missingProjectPaths?: string[];
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
  description?: string;
  source?: unknown;
  url?: string;
  image_url?: string;
  filename?: string;
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
