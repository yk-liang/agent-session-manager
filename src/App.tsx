import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FolderOpen } from "lucide-react";
import UiIcon from "./components/UiIcon";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Sidebar from "./components/Sidebar";
import AgentSwitcher from "./components/AgentSwitcher";
import SessionList from "./components/SessionList";
import SessionDetailView from "./components/SessionDetail";
import SessionDeleteDialog, { type SessionDeletion, type SessionDeleteTarget } from "./components/SessionDeleteDialog";
import BatchDeleteProgress from "./components/BatchDeleteProgress";
import ConfigPanel from "./components/ConfigPanel";
import UsagePanel from "./usage/UsagePanel";
import { findRequestSession } from "./usage/requestSession";
import type { RequestEvent } from "./usage/types";
import ConfirmDialog from "./components/ConfirmDialog";
import ResizableLayout from "./components/ResizableLayout";
import ReadDiagnostics from "./components/ReadDiagnostics";
import * as api from "./api";
import { displaySessionId, isEmptySession, sessionKey } from "./utils";
import { useConversationSearch } from "./useConversationSearch";
import { catalogGroups } from "./sessionGroups";
import { useSessionCatalog, useSessionCounts } from "./useSessionCatalog";
import { catalogFilterFor, DEFAULT_CATALOG_FILTER } from "./catalogCache";
import { useUiPreference } from "./useUiPreference";
import type { SessionReading } from "./sessionContent";
import type { AgentInfo, CatalogFilter, ConfigActivity, ConversationSearchTarget, SessionDeleteFailure, SessionDetail, SessionMeta } from "./types";

const NO_SESSIONS: SessionMeta[] = [];
const NO_AGENTS: AgentInfo[] = [];

// 目录预览的文件时间与最终统计口径不同。
const sessionContentStamp = (session: SessionMeta) => `${session.sizeBytes ?? ""}|${session.lastTs ?? ""}`;

const sameSearchTarget = (a: ConversationSearchTarget | null, b: ConversationSearchTarget | null) =>
  a === b || !!a && !!b && a.query === b.query && a.eventIndex === b.eventIndex
    && a.eventId === b.eventId && a.snippet === b.snippet && a.role === b.role;

type Tab = "sessions" | "config" | "usage";

type SessionOperation =
  | { kind: "confirmDelete"; request: SessionDeletion }
  | { kind: "delete"; progress: { completed: number; total: number } | null }
  | { kind: "archive" };

export default function App() {
  const [tab, setTab] = useState<Tab>("sessions");
  const [detectedAgents, setAgents] = useState<AgentInfo[] | null>(null);
  const agents = detectedAgents ?? NO_AGENTS;
  const [preferredAgents, setPreferredAgents] = useUiPreference("agents");
  const [selectedProject, setSelectedProject] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [loadedSession, setLoadedSession] = useState<{ detail: SessionDetail; reading: SessionReading } | null>(null);
  const detail = loadedSession?.detail ?? null;
  const [readRequest, setReadRequest] = useState<{ session: SessionMeta; pending: boolean; error: string | null } | null>(null);
  const [searchTarget, setSearchTarget] = useState<ConversationSearchTarget | null>(null);
  const [filter, setFilter] = useUiPreference("sessionList");
  const [requestReveal, setRequestReveal] = useState(DEFAULT_CATALOG_FILTER);
  const [configRefreshKey, setConfigRefreshKey] = useState(0);
  const [batchMode, setBatchMode] = useState(false);
  const [checkedSessionKeys, setCheckedSessionKeys] = useState<Set<string>>(new Set());
  const [expandedSessions, setExpandedSessions] = useState<Set<string>>(new Set());
  const [revealKey, setRevealKey] = useState<string | null>(null);
  const [operation, setOperation] = useState<SessionOperation | null>(null);
  const [batchFailures, setBatchFailures] = useState<SessionDeleteFailure[]>([]);
  const mutationRunning = useRef(false);
  const deleting = useRef(false);
  const deleteRun = useRef(0);
  const batchDeleteProgress = operation?.kind === "delete" ? operation.progress : null;
  const sessionActionsBusy = operation !== null;
  const [toast, setToast] = useState<{ msg: string; error: boolean } | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);
  const detailRequestGeneration = useRef(0);
  const closeSessionDetail = useCallback(() => {
    detailRequestGeneration.current++;
    setLoadedSession(null);
    setReadRequest(null);
    setSearchTarget(null);
    setRevealKey(null);
  }, []);
  const [configActivity, setConfigActivity] = useState<ConfigActivity>({ dirty: false, busy: false });
  const configActivityRef = useRef(configActivity);
  const [configNavigation, setConfigNavigation] = useState<{ label: string; action: () => void } | null>(null);
  const onConfigActivity = useCallback((activity: ConfigActivity) => {
    configActivityRef.current = activity;
    setConfigActivity((current) => current.dirty === activity.dirty && current.busy === activity.busy ? current : activity);
  }, []);

  const showToast = useCallback((msg: string, error = false) => {
    setToast({ msg, error });
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 3000);
  }, []);

  useEffect(() => {
    if (isTauri()) void api.setConfigGuard(configActivity).catch(() => showToast("暂时无法检查未保存的配置。", true));
  }, [configActivity, showToast]);

  useEffect(() => {
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (deleting.current || configActivityRef.current.dirty || configActivityRef.current.busy) {
        event.preventDefault(); event.returnValue = "";
      }
    };
    window.addEventListener("beforeunload", beforeUnload);
    const unlisten = isTauri() ? listen("config-exit-requested", () => {
      if (deleting.current) return;
      const activity = configActivityRef.current;
      if (activity.busy) { showToast("配置操作仍在进行，请稍后退出。", true); return; }
      const exit = () => { void api.confirmConfigExit().catch((e) => showToast(String(e), true)); };
      if (activity.dirty) setConfigNavigation({ label: "退出应用", action: exit }); else exit();
    }) : null;
    return () => { window.removeEventListener("beforeunload", beforeUnload); void unlisten?.then((off) => off()); };
  }, [showToast]);

  const protectConfig = (action: () => void, label: string) => {
    const activity = configActivityRef.current;
    if (tab === "config" && activity.busy) { showToast("请等待配置操作完成。", true); return; }
    if (tab === "config" && activity.dirty) { setConfigNavigation({ label, action }); return; }
    action();
  };

  useEffect(() => {
    let active = true;
    api
      .listAgents()
      .then(list => { if (active) setAgents(list); })
      .catch((e) => {
        if (!active) return;
        setAgents(NO_AGENTS);
        showToast(String(e), true);
      });
    return () => { active = false; };
  }, [showToast]);

  const availableAgents = agents.filter((agent) => agent.installed);
  const currentAgent = availableAgents.find((agent) => agent.id === preferredAgents.local) ?? availableAgents[0];
  const selectedAgent = currentAgent?.id ?? null;
  const usageAgent = availableAgents.some(agent => agent.id === preferredAgents.usage) ? preferredAgents.usage : "all";
  const supportsArchivedSessions = currentAgent?.supportsArchivedSessions;
  const catalogFilter = useMemo(
    () => catalogFilterFor({
      includeEmpty: filter.includeEmpty || requestReveal.includeEmpty,
      includeArchived: filter.includeArchived || requestReveal.includeArchived,
    }, supportsArchivedSessions),
    [filter, requestReveal, supportsArchivedSessions],
  );
  const { includeArchived } = catalogFilter;
  const onWatchError = useCallback(() => showToast("会话可能无法自动更新。切回应用可重试。", true), [showToast]);
  const { cache: catalogCache, catalog, loading, refreshing, error: catalogError, refresh: refreshCatalog } = useSessionCatalog(
    currentAgent, catalogFilter, tab === "sessions", sessionActionsBusy || batchMode, onWatchError,
  );
  const sessionCounts = useSessionCounts(availableAgents, filter, catalogCache, selectedAgent, catalogFilter);
  const [requestCounts, setRequestCounts] = useState<ReadonlyMap<string, number | null>>(() => new Map());
  const catalogWarnings = catalog.warnings;
  const batchBusy = loading || refreshing;

  const clearBatchSelection = () => {
    setCheckedSessionKeys(new Set());
    setBatchFailures([]);
  };

  const selectSessionScope = (project: string | null) => {
    clearBatchSelection();
    setBatchMode(false);
    closeSessionDetail();
    setSelectedProject(project);
  };

  const changeTab = (next: Tab) => {
    if (next === tab || sessionActionsBusy || mutationRunning.current) return;
    protectConfig(() => {
      clearBatchSelection();
      setBatchMode(false);
      if (tab === "usage") setRequestCounts(new Map());
      setTab(next);
    }, "切换标签页");
  };

  const changeSearch = (next: string) => {
    if (sessionActionsBusy || mutationRunning.current) return;
    clearBatchSelection();
    setSearch(next);
  };

  const selectAgent = (id: string) => {
    if (sessionActionsBusy || mutationRunning.current || id === (tab === "usage" ? usageAgent : selectedAgent)) return;
    const available = availableAgents.some(agent => agent.id === id);
    if (!available && !(tab === "usage" && id === "all")) return;
    protectConfig(() => {
      if (available && id !== selectedAgent) {
        selectSessionScope(null);
        setExpandedSessions(new Set());
        setRequestReveal(DEFAULT_CATALOG_FILTER);
      }
      setPreferredAgents(current => ({ local: available ? id : current.local, usage: id }));
    }, "切换 Agent");
  };

  const selectProject = (dirName: string | null) => {
    if (sessionActionsBusy || mutationRunning.current) return;
    selectSessionScope(dirName);
  };

  const { groups: sidebarGroups, byGroup, hints: groupHints, projectlessKeys: hiddenPathKeys, tree } = catalogGroups(catalog);
  const scopedSessions = selectedProject === null ? catalog.sessions : byGroup.get(selectedProject) ?? NO_SESSIONS;
  useEffect(() => {
    if (tab !== "sessions" || loading || sessionActionsBusy || selectedProject === null || sidebarGroups.some((group) => group.dirName === selectedProject)) return;
    closeSessionDetail();
    setSelectedProject(null);
    setBatchMode(false);
    setCheckedSessionKeys(new Set());
  }, [sidebarGroups, selectedProject, loading, sessionActionsBusy, tab, closeSessionDetail]);
  const query = search.trim();
  const contentSearch = useConversationSearch(selectedAgent, query, scopedSessions, tab === "sessions" && !loading);

  const filtered = useMemo(() => {
    const q = query.toLowerCase();
    if (!q) return scopedSessions;
    return scopedSessions.filter((s) => {
      return contentSearch.hits.has(sessionKey(s)) || [s.title, displaySessionId(s), s.cwd, s.model, s.codex?.source, s.codex?.threadSource]
        .filter(Boolean)
        .some((v) => String(v).toLowerCase().includes(q));
    });
  }, [scopedSessions, query, contentSearch.hits]);
  const scopeKeys = useMemo(() => new Set(scopedSessions.map(sessionKey)), [scopedSessions]);
  const activeRevealKey = !query && revealKey && scopeKeys.has(revealKey) ? revealKey : null;
  const expanded = useMemo(() => activeRevealKey
    ? new Set([...expandedSessions, ...tree.ancestors(activeRevealKey, scopeKeys)])
    : expandedSessions, [activeRevealKey, expandedSessions, tree, scopeKeys]);
  const sessionRows = useMemo(() => tree.rows(filtered, expanded, Boolean(query)), [tree, filtered, expanded, query]);
  const visibleKeys = useMemo(() => new Set(sessionRows.map(row => row.key)), [sessionRows]);

  useEffect(() => {
    if (!batchMode || batchBusy || !checkedSessionKeys.size) return;
    const remaining = [...checkedSessionKeys].filter(key => visibleKeys.has(key));
    if (remaining.length !== checkedSessionKeys.size) setCheckedSessionKeys(new Set(remaining));
  }, [batchMode, batchBusy, visibleKeys, checkedSessionKeys]);

  const selectedDetail = useMemo(() => {
    if (tab !== "sessions" || !detail || !selectedAgent) return detail;
    const meta = catalogCache.withCurrentMetadata(selectedAgent, catalogFilter, detail.meta);
    return meta === detail.meta ? detail : { ...detail, meta };
  }, [catalog, catalogCache, catalogFilter, detail, selectedAgent, tab]);

  const selectedSessionKey = detail ? sessionKey(detail.meta) : readRequest ? sessionKey(readRequest.session) : null;
  const hidingSessionKey = !query && selectedSessionKey && scopeKeys.has(selectedSessionKey)
    ? tree.ancestors(selectedSessionKey, scopeKeys).reverse().find(key => !expanded.has(key))
    : undefined;
  const parent = selectedDetail ? tree.parent(selectedDetail.meta) : undefined;

  const toggleChildren = (key: string) => {
    setRevealKey(null);
    const next = new Set(expanded);
    if (!next.delete(key)) next.add(key);
    setExpandedSessions(next);
  };

  const completeReveal = useCallback(() => {
    if (!activeRevealKey) return;
    setExpandedSessions(expanded);
    setRevealKey(key => key === activeRevealKey ? null : key);
  }, [activeRevealKey, expanded]);

  function openParentSession() {
    if (!parent || !selectedAgent || sessionActionsBusy || mutationRunning.current) return;
    const key = sessionKey(parent);
    if (!scopeKeys.has(key)) selectSessionScope(null);
    changeSearch("");
    setRevealKey(key);
    void loadSession(selectedAgent, parent, { matchSearch: false });
  }

  const changeFilter = (next: CatalogFilter) => {
    if (sessionActionsBusy || mutationRunning.current) return;
    clearBatchSelection();
    const changedEmpty = next.includeEmpty !== catalogFilter.includeEmpty;
    const changedArchived = supportsArchivedSessions && next.includeArchived !== catalogFilter.includeArchived;
    setRequestReveal(current => ({
      includeEmpty: !changedEmpty && current.includeEmpty,
      includeArchived: !changedArchived && current.includeArchived,
    }));
    setFilter({
      includeEmpty: changedEmpty ? next.includeEmpty : filter.includeEmpty,
      includeArchived: changedArchived ? next.includeArchived : filter.includeArchived,
    });
    if (changedArchived && !next.includeArchived && (selectedDetail?.meta ?? readRequest?.session)?.archived) closeSessionDetail();
  };

  const openSession = (session: SessionMeta, toggleSelection = false) => {
    if (!selectedAgent || sessionActionsBusy || mutationRunning.current || (toggleSelection && batchBusy)) return;
    setRevealKey(query && session.subagent ? sessionKey(session) : null);
    if (toggleSelection) {
      const key = sessionKey(session);
      setCheckedSessionKeys(current => {
        const next = new Set(current);
        if (!next.delete(key)) next.add(key);
        return next;
      });
    }
    void loadSession(selectedAgent, session);
  };

  // 若打开会话时目录统计尚未完成，首次完成时只建立基线。
  const detailContentVersion = useRef<{ key: string; stamp: string | null; refreshingStamp?: string } | null>(null);
  const loadSession = async (agentId: string, s: SessionMeta,
    { refreshStamp, matchSearch = true }: { refreshStamp?: string; matchSearch?: boolean } = {}) => {
    const generation = ++detailRequestGeneration.current;
    const key = sessionKey(s);
    const keep = tab === "sessions" && agentId === selectedAgent && !!detail && sessionKey(detail.meta) === key;
    const refresh = refreshStamp !== undefined;
    const hit = refresh || !matchSearch || tab !== "sessions" ? undefined : contentSearch.hits.get(key);
    const target = hit ? { ...hit, query } : null;
    if (!refresh) {
      const row = agentId === selectedAgent && tab === "sessions" ? tree.byKey.get(key) : s;
      detailContentVersion.current = { key, stamp: row && !row.statsPending ? sessionContentStamp(row) : null };
      if (!keep) setLoadedSession(null);
      setReadRequest(current => ({
        session: s, pending: true,
        error: current && sessionKey(current.session) === key ? current.error : null,
      }));
    }
    try {
      const d = await api.readSession(agentId, s.projectDir, s.sessionId);
      if (generation !== detailRequestGeneration.current) return;
      catalogCache.setReadError(agentId, s, null);
      if (refresh) {
        detailContentVersion.current = { key, stamp: refreshStamp };
      } else {
        setReadRequest(null);
        if (tab === "usage" && s.statsPending && isEmptySession(d.meta)) {
          setRequestReveal(current => ({ ...current, includeEmpty: true }));
        }
        // 保持定位对象稳定，避免刷新时跳回命中处。
        setSearchTarget(current => keep && sameSearchTarget(current, target) ? current : target);
      }
      setLoadedSession(current => ({
        detail: d,
        reading: keep && current ? current.reading : { expanded: new Map(), payloads: new Map() },
      }));
    } catch (e) {
      if (generation !== detailRequestGeneration.current) return;
      const error = String(e);
      if (refresh) {
        const known = detailContentVersion.current;
        if (known?.key === key) detailContentVersion.current = { key, stamp: known.stamp };
        return;
      }
      if (keep) {
        setReadRequest(null);
        showToast("刷新会话失败，已保留上次内容。", true);
        return;
      }
      catalogCache.setReadError(agentId, s, error);
      setReadRequest({ session: s, pending: false, error });
    }
  };

  const openRequestSession = async (event: RequestEvent, signal: AbortSignal) => {
    const agent = availableAgents.find(agent => agent.id === event.agent);
    if (!agent || tab !== "usage" || sessionActionsBusy || mutationRunning.current) return;

    try {
      const scope = catalogFilterFor({ includeEmpty: true, includeArchived: true }, agent.supportsArchivedSessions);
      const source = await catalogCache.load(agent.id, scope, api.nextCatalogGeneration());
      if (signal.aborted) return;
      const session = findRequestSession(source, event);
      if (!session) {
        showToast("未找到可打开的关联会话。", true);
        return;
      }
      selectSessionScope(null);
      setSearch("");
      setRequestReveal({ includeEmpty: !session.statsPending && isEmptySession(session), includeArchived: !!session.archived });
      if (agent.id !== selectedAgent) setExpandedSessions(new Set());
      if (session.subagent) setRevealKey(sessionKey(session));
      setPreferredAgents(current => ({ ...current, local: agent.id }));
      changeTab("sessions");
      void loadSession(agent.id, session);
    } catch (error) {
      if (!signal.aborted) showToast(`无法查找关联会话：${String(error)}`, true);
    }
  };

  // 确认会话文件有变化后再刷新，避免与目录统计同时读取大日志。
  useEffect(() => {
    if (tab !== "sessions" || !detail || !selectedAgent || readRequest || sessionActionsBusy) return;
    const row = tree.byKey.get(sessionKey(detail.meta));
    if (!row || row.statsPending) return;
    const key = sessionKey(row);
    const stamp = sessionContentStamp(row);
    const known = detailContentVersion.current;
    if (known?.key !== key || known.stamp === null) {
      detailContentVersion.current = { key, stamp };
      return;
    }
    if (known.stamp === stamp || known.refreshingStamp === stamp) return;
    detailContentVersion.current = { ...known, refreshingStamp: stamp };
    void loadSession(selectedAgent, row, { refreshStamp: stamp });
  }, [tree, detail, readRequest, selectedAgent, sessionActionsBusy, tab]);

  const toggleBatch = () => {
    if (sessionActionsBusy || batchBusy || mutationRunning.current) return;
    setBatchMode(!batchMode);
    clearBatchSelection();
  };

  const toggleAll = () => {
    if (sessionActionsBusy || batchBusy) return;
    const allSelected = [...visibleKeys].every(key => checkedSessionKeys.has(key));
    setCheckedSessionKeys(allSelected ? new Set() : new Set(visibleKeys));
  };

  const requestDeletion = (target: SessionDeleteTarget) => {
    if (!currentAgent || currentAgent.deletePolicy === "readOnly" || sessionActionsBusy
      || mutationRunning.current || !target.sessions.length) return;
    setOperation({ kind: "confirmDelete", request: { ...target, agent: currentAgent } });
  };

  const requestSessionDelete = (session: SessionMeta) => requestDeletion({ mode: "single", sessions: [session] });

  const toggleSessionArchive = async (session: SessionMeta) => {
    if (!currentAgent?.supportsArchive || sessionActionsBusy || mutationRunning.current) return;
    const archived = !session.archived;
    mutationRunning.current = true;
    setOperation({ kind: "archive" });
    try {
      const result = await api.setSessionArchived(currentAgent.id, session, archived);
      const changed = result?.changed ?? [{ session, archived }];
      const targetKey = sessionKey(session);
      catalogCache.setArchived(currentAgent.id, changed.map(change => ({
        ...change,
        meta: sessionKey(change.session) === targetKey ? session : undefined,
      })));
      const selectedChange = changed.find(change => sessionKey(change.session) === selectedSessionKey);
      if (selectedChange) {
        const meta = tree.byKey.get(sessionKey(selectedChange.session));
        if (selectedChange.archived && !includeArchived) closeSessionDetail();
        else if (meta) void loadSession(currentAgent.id, { ...meta, archived: selectedChange.archived });
      }
      if (result?.error) throw new Error(result.error);
      showToast(archived ? "会话已归档" : "已取消归档");
    } catch (error) {
      showToast(String(error), true);
    } finally {
      mutationRunning.current = false;
      setOperation(null);
      refreshCatalog();
    }
  };

  const requestBatchDelete = () => {
    if (batchBusy) return;
    const selectedRows = sessionRows.filter(row => checkedSessionKeys.has(row.key));
    const selectedKeys = new Set(selectedRows.map(row => row.key));
    const roots = selectedRows.filter(row => !tree.ancestors(row.key).some(parent => selectedKeys.has(parent)));
    requestDeletion({ mode: "batch", sessions: roots.map(row => row.session) });
  };

  const confirmDeletion = async () => {
    if (operation?.kind !== "confirmDelete" || mutationRunning.current) return;
    const { agent, sessions: targets, mode } = operation.request;
    const batch = mode === "batch";
    const total = targets.length;
    const useNativeGuard = isTauri();
    mutationRunning.current = true;
    deleting.current = true;
    const run = ++deleteRun.current;
    setOperation({ kind: "delete", progress: batch ? { completed: 0, total } : null });
    try {
      if (useNativeGuard) {
        try { await api.setDeleteGuard(true); }
        catch (error) { showToast(`无法启用${batch ? "批量" : ""}删除保护：${String(error)}`, true); return; }
      }
      if (batch) setBatchFailures([]);
      // 进度经通道推送，可能晚于删除结果到达，只更新本次仍在进行的删除。
      const result = await api.deleteSessions(agent.id, targets, completed => {
        if (batch && run === deleteRun.current) {
          setOperation(current => current?.kind === "delete" ? { kind: "delete", progress: { completed, total } } : current);
        }
      }).catch(error => ({ removed: [], failures: targets.map(session => ({ session, error: String(error) })) }));
      const requested = new Map(targets.map(session => [sessionKey(session), session]));
      const failures = result.failures.flatMap(({ session, error }) => {
        const meta = requested.get(sessionKey(session));
        return meta ? [{ session: meta, error }] : [];
      });
      const removed = new Map(result.removed.map(session => [sessionKey(session), session]));
      if (removed.size) {
        if (selectedSessionKey && removed.has(selectedSessionKey)) closeSessionDetail();
        catalogCache.remove(agent.id, [...removed.values()]);
      }
      const toTrash = agent.deletePolicy === "trash";
      if (batch) {
        setCheckedSessionKeys(new Set(failures.map(({ session }) => sessionKey(session))));
        setBatchFailures(failures);
        const completed = removed.size
          ? `${removed.size} 个会话${toTrash ? "已移入废纸篓" : "已删除"}`
          : "";
        const failed = failures.length ? `${failures.length} 项删除失败` : "";
        showToast([completed, failed].filter(Boolean).join("，"), failures.length > 0);
      } else if (failures.length) {
        showToast(failures[0].error, true);
      } else {
        showToast(toTrash ? "会话已移入废纸篓" : "会话已删除");
      }
      refreshCatalog();
    } finally {
      if (useNativeGuard) await api.setDeleteGuard(false)
        .catch(error => showToast(`无法解除删除保护：${String(error)}`, true));
      deleting.current = false;
      setOperation(null);
      mutationRunning.current = false;
    }
  };

  return (
    <div className="app" inert={batchDeleteProgress !== null}>
      <header className="topbar">
        <AgentSwitcher agents={availableAgents} value={tab === "usage" ? usageAgent : selectedAgent} allowAll={tab === "usage"}
          counts={tab === "usage" ? requestCounts : tab === "sessions" ? sessionCounts.counts : undefined}
          onOpen={tab === "sessions" ? sessionCounts.refresh : undefined}
          disabled={sessionActionsBusy || (tab === "config" && configActivity.busy)} onChange={selectAgent} />
        <nav className="tabs">
          <button type="button" className="btn-ghost" aria-current={tab === "sessions" ? "page" : undefined} onClick={() => changeTab("sessions")} disabled={sessionActionsBusy}>
            会话
          </button>
          {availableAgents.length > 0 && (
            <button type="button" className="btn-ghost" aria-current={tab === "config" ? "page" : undefined} onClick={() => changeTab("config")} disabled={sessionActionsBusy}>
              配置
            </button>
          )}
          <button type="button" className="btn-ghost" aria-current={tab === "usage" ? "page" : undefined} onClick={() => changeTab("usage")} disabled={sessionActionsBusy}>用量</button>
        </nav>
        <div className="topbar-actions">
          {tab !== "usage" && currentAgent?.dataDir && (
            <button
              type="button"
              className="button-with-icon"
              onClick={() =>
                api.openDataDir(currentAgent.id).catch((e) => showToast(`无法打开数据目录：${e}`, true))
              }
              title={`在访达中打开 ${currentAgent.displayName} 数据目录`}
            >
              <UiIcon icon={FolderOpen} />打开数据目录
            </button>
          )}
        </div>
      </header>

      {tab === "sessions" ? (
        <ResizableLayout
          sidebar={<Sidebar
            selectedAgent={selectedAgent}
            projects={sidebarGroups}
            selected={selectedProject}
            onSelect={selectProject}
            loading={loading}
            disabled={sessionActionsBusy}
            catalogIncomplete={!!catalogError || catalogWarnings.length > 0}
          />}
          sessionList={<SessionList
            scopeKey={JSON.stringify([selectedAgent, selectedProject, catalogFilter])}
            rows={sessionRows}
            expanded={expanded}
            onToggleChildren={toggleChildren}
            hidingSessionKey={hidingSessionKey}
            revealSessionKey={activeRevealKey}
            onRevealComplete={completeReveal}
            selectedSessionKey={selectedSessionKey}
            search={search}
            onSearch={changeSearch}
            onSelect={session => openSession(session, batchMode)}
            filter={catalogFilter}
            onFilterChange={changeFilter}
            batchMode={batchMode}
            agent={currentAgent}
            onRequestDelete={requestSessionDelete}
            onToggleArchive={toggleSessionArchive}
            toast={showToast}
            checkedSessionKeys={checkedSessionKeys}
            onToggleBatch={toggleBatch}
            onToggleAll={toggleAll}
            onDeleteSelected={requestBatchDelete}
            disabled={sessionActionsBusy}
            loading={loading}
            refreshing={refreshing}
            loadError={catalogError}
            warnings={catalogWarnings}
            onRetryLoad={refreshCatalog}
            failures={batchFailures}
            searchHits={contentSearch.hits}
            searchPending={contentSearch.pending}
            searchSkipped={contentSearch.skipped}
            searchError={contentSearch.error}
            onRetrySearch={contentSearch.retry}
            showPaths={selectedProject === null}
            hiddenPathSessionKeys={hiddenPathKeys}
            displayRoots={groupHints}
          />}
        >
          {selectedDetail && currentAgent && loadedSession ? (
            <SessionDetailView
              detail={selectedDetail}
              reading={loadedSession.reading}
              agent={currentAgent}
              onRequestDelete={requestSessionDelete}
              onToggleArchive={toggleSessionArchive}
              onOpenParent={parent ? openParentSession : undefined}
              onReveal={hidingSessionKey && selectedSessionKey ? () => setRevealKey(selectedSessionKey) : undefined}
              actionsDisabled={sessionActionsBusy}
              toast={showToast}
              searchTarget={searchTarget?.query === query ? searchTarget : null}
              onRetryRead={() => void openSession(selectedDetail.meta)}
            />
          ) : readRequest ? (
            <div className="detail-read-state" aria-busy={readRequest.pending}>
              {readRequest.error ? <ReadDiagnostics
                title={/^stage: (parse_json|decode_utf8)$/m.test(readRequest.error) ? "无法解析会话正文" : "无法读取会话正文"}
                errors={[readRequest.error]}
                retry={() => void openSession(readRequest.session)}
                busy={readRequest.pending}
              /> : <p role="status">正在读取会话…</p>}
            </div>
          ) : (
            <div className="placeholder">选择会话以查看内容</div>
          )}
        </ResizableLayout>
      ) : tab === "config" ? (
        <main className="main single">
          {selectedAgent ? (
            <ConfigPanel
              key={selectedAgent}
              agent={selectedAgent}
              displayName={currentAgent?.displayName ?? selectedAgent}
              refreshKey={configRefreshKey}
              onRetry={() => protectConfig(() => setConfigRefreshKey(key => key + 1), "重新读取配置")}
              toast={showToast}
              onActivityChange={onConfigActivity}
            />
          ) : (
            <div className="placeholder">选择 Agent 以查看配置</div>
          )}
        </main>
      ) : detectedAgents === null ? <main className="main single"><div className="placeholder">正在读取 Agent…</div></main>
        : <UsagePanel agents={agents} agent={usageAgent} onRequestCounts={setRequestCounts} onOpenSession={openRequestSession} showToast={showToast} />}

      {configNavigation && <ConfirmDialog title="放弃未保存的修改？" message={`${configNavigation.label}后，未保存的配置将丢失。`} confirmLabel="放弃修改" onCancel={() => setConfigNavigation(null)} onConfirm={() => {
        const action = configNavigation.action;
        setConfigNavigation(null); action();
      }} />}

      {operation?.kind === "confirmDelete" && <SessionDeleteDialog
        deletion={operation.request}
        onConfirm={confirmDeletion}
        onCancel={() => setOperation(current => current?.kind === "confirmDelete" ? null : current)}
      />}
      {batchDeleteProgress && <BatchDeleteProgress {...batchDeleteProgress} />}
      <div className={toast ? toast.error ? "toast error" : "toast" : undefined}
        role="status" aria-live="polite" aria-atomic="true">
        {toast?.msg}
      </div>
    </div>
  );
}
