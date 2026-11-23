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
  script_result?: ContentBlock;
  /** 后端给出的操作描述；延迟加载时只带标题所需字段，正文随原始数据一起读取。 */
  ops?: ToolOp[];
}

/** 行以空格、`-`、`+` 开头；起始行号未知时为空。 */
export interface DiffHunk {
  old_start: number | null;
  new_start: number | null;
  lines: string[];
}

/** 一个文件里的搜索命中；`context` 是 `-A`、`-B` 带出的上下文行。 */
export interface SearchHits {
  path: string;
  lines: { line?: number; text: string; context?: boolean }[];
}

export type BrowseOp =
  | { kind: "read"; path: string; start?: number; end?: number; total?: number; tail?: number }
  | { kind: "search"; pattern: string; paths?: string[]; in_file?: boolean; matches?: number; files?: number; hits?: SearchHits[] }
  | { kind: "find"; pattern?: string; paths?: string[]; count?: number }
  | { kind: "list"; paths?: string[]; count?: number };

export type ChangeOp =
  | { kind: "edit"; path: string; added: number; removed: number; hunks?: DiffHunk[] }
  | { kind: "write"; path: string; lines: number; created?: boolean; content?: string }
  | { kind: "delete"; path: string };

export type ShellFilter =
  | { kind: "head" | "tail"; value: number }
  | { kind: "include" | "exclude"; value: string }
  | { kind: "count" };

export type ShellIntent =
  | "read" | "search" | "find" | "list" | "write" | "modify" | "script" | "run"
  | "git_diff" | "git_log" | "git_commit" | "git" | "github"
  | "test" | "build" | "install" | "remote" | "http" | "browser" | "database"
  | "mkdir" | "remove" | "copy" | "move" | "process" | "kill" | "wait" | "open" | "count" | "other";

/** heredoc、`-c`/`-e` 参数或脚本工具的代码；`first` 是第一行有意义的代码。 */
export interface ScriptInfo {
  lang?: string;
  title?: string;
  lines: number;
  first: string;
  body?: string;
}

/** 只读、搜、找、列的命令带 `steps`，按文件操作展示；其余命令按第一步的意图展示。 */
export interface ShellOp {
  kind: "shell";
  command?: string;
  description?: string;
  exit_code?: number;
  no_match?: boolean;
  cwd?: string;
  intent: ShellIntent;
  head?: string;
  filters?: ShellFilter[];
  more?: number;
  looped?: boolean;
  target?: string;
  append?: boolean;
  script?: ScriptInfo;
  steps?: (BrowseOp & { program: string; filters?: ShellFilter[] })[];
}

export interface AskQuestion {
  question: string;
  header?: string;
  options: { label: string; description?: string }[];
  answer?: string;
}

export type ToolOp =
  | BrowseOp
  | ChangeOp
  | ShellOp
  | ({ kind: "script" } & ScriptInfo)
  | { kind: "skill"; name: string; args?: string; path?: string; description?: string; body?: string }
  | { kind: "web_search"; query: string; domain?: string; results?: number }
  | { kind: "web_fetch"; url: string; prompt?: string }
  | { kind: "ask"; questions: AskQuestion[] }
  | { kind: "agent"; agent?: string; task: string; background?: boolean }
  | { kind: "message"; to: string; summary?: string }
  | { kind: "wait"; seconds?: number; target?: string }
  | { kind: "wake"; seconds: number; reason?: string }
  | { kind: "stop"; task: string; detail?: string }
  | { kind: "monitor"; description: string; minutes?: number }
  | { kind: "load_tools"; names: string[] }
  | { kind: "todo"; items: { text: string; status: string }[] }
  | { kind: "image"; path: string }
  | { kind: "mcp"; server: string; tool: string; title?: string };

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
