import { invoke } from "@tauri-apps/api/core";
import type {
  AgentInfo,
  Catalog,
  CurrentConfig,
  ProfileSummary,
  SessionDetail,
  ConfigWorkspace,
  ConfigValidation,
  ConfigChangeKind,
  ConfigChangePreview,
  ConfigActivity,
  ConversationSearchResult,
  SessionMeta,
} from "./types";

export const listAgents = () => invoke<AgentInfo[]>("list_agents");
export const openDataDir = (agent: string) => invoke<void>("open_data_dir", { agent });

let usageGeneration = Date.now();
export const nextUsageGeneration = () => ++usageGeneration;
export const cancelUsage = () => invoke<void>("cancel_usage", { generation: nextUsageGeneration() });
export const syncRequestEvents = (agent: string, generation: number) => invoke<import("./types").UsageSync>("sync_request_events", { agent, generation });
let summaryGeneration = Date.now();
export const nextSummaryGeneration = () => ++summaryGeneration;
export const readUsageSummary = (query: import("./types").UsageSummaryQuery, generation: number) => invoke<import("./types").UsageSummary>("read_usage_summary", { query, generation });
export const cancelUsageSummary = () => invoke<void>("cancel_usage_summary", { generation: nextSummaryGeneration() });
let eventGeneration = Date.now();
export const nextEventGeneration = () => ++eventGeneration;
export const readRequestEvents = (query: import("./types").EventQuery, generation: number) =>
  invoke<import("./types").EventPage>("read_request_events", { query, generation });
export const cancelRequestEvents = () => invoke<void>("cancel_request_events", { generation: nextEventGeneration() });
export const collectorStatus = () => invoke<import("./types").CollectorStatus>("collector_status");
export const setCollectorEnabled = (enabled: boolean) => invoke<import("./types").CollectorStatus>("set_collector_enabled", { enabled });
export const openCollectorSettings = () => invoke<void>("open_collector_settings");

export const catalog = (agent: string, includeEmpty = false) =>
  invoke<Catalog>("catalog", { agent, includeEmpty });

let refinementGeneration = Date.now();
export const nextRefinementGeneration = () => ++refinementGeneration;
export const refineCatalog = (agent: string, includeEmpty: boolean, generation: number) =>
  invoke<Catalog>("refine_catalog", { agent, includeEmpty, generation });
export const cancelCatalogRefinement = () =>
  invoke<void>("cancel_catalog_refinement", { generation: nextRefinementGeneration() });
export const readToolPayload = (token: string) => invoke<import("./types").ContentBlock>("read_tool_payload", { token });

export const resolveModelNames = (agent: string, models: string[]) =>
  invoke<Record<string, string>>("resolve_model_names", { agent, models });

export const watchSessions = (agent: string | null, generation: number) =>
  invoke<void>("watch_sessions", { agent, generation });

export const readSession = (
  agent: string,
  projectDir: string,
  sessionId: string,
) => invoke<SessionDetail>("read_session", { agent, projectDir, sessionId, view: true });

export const searchConversations = (agent: string, query: string, sessions: SessionMeta[]) =>
  invoke<ConversationSearchResult>("search_conversations", {
    agent, query,
    targets: sessions.map(({ projectDir, sessionId }) => ({ projectDir, sessionId })),
  });

export const cancelConversationSearch = () => invoke<void>("cancel_conversation_search");

export const deleteSession = (
  agent: string,
  projectDir: string,
  sessionId: string,
) => invoke<void>("delete_session", { agent, projectDir, sessionId });

export const resumeCommand = (agent: string, sessionId: string, fork = false) =>
  invoke<string | null>("resume_command", { agent, sessionId, fork });

export const resumeInTerminal = (
  agent: string,
  sessionId: string,
  cwd?: string,
  fork = false,
) =>
  invoke<void>("resume_in_terminal", {
    agent,
    sessionId,
    cwd: cwd ?? null,
    fork,
  });

export const getCurrentConfig = (agent: string) =>
  invoke<CurrentConfig>("get_current_config", { agent });

export const listProfiles = (agent: string) =>
  invoke<ProfileSummary[]>("list_profiles", { agent });

export const saveProfile = (
  agent: string,
  name: string,
  expectedRevision: string,
) => invoke<ProfileSummary>("save_profile", { agent, name, expectedRevision });

export const deleteProfile = (agent: string, name: string) =>
  invoke<void>("delete_profile", { agent, name });

export const updateConfigFile = (
  agent: string,
  key: string,
  content: string,
  expectedRevision: string,
) =>
  invoke<void>("update_config_file", { agent, key, content, expectedRevision });

export const getConfigWorkspace = (agent: string) =>
  invoke<ConfigWorkspace>("get_config_workspace", { agent });

export const validateConfigContent = (
  agent: string,
  key: string,
  content: string,
) =>
  invoke<ConfigValidation>("validate_config_content", { agent, key, content });

export const previewConfigChange = (
  agent: string,
  kind: ConfigChangeKind,
  target: string,
) =>
  invoke<ConfigChangePreview>("preview_config_change", { agent, kind, target });

export const applyConfigChange = (
  agent: string,
  preview: ConfigChangePreview,
) =>
  invoke<void>("apply_config_change", {
    agent,
    kind: preview.kind,
    target: preview.target,
    expectedCurrent: preview.currentRevision,
    expectedTarget: preview.targetRevision,
  });

export const setConfigGuard = (activity: ConfigActivity) =>
  invoke<void>("set_config_guard", {
    dirty: activity.dirty,
    busy: activity.busy,
  });
export const confirmConfigExit = () => invoke<void>("confirm_config_exit");
