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
import ConfigPanel from "./components/ConfigPanel";
import UsagePanel from "./components/UsagePanel";
import ConfirmDialog from "./components/ConfirmDialog";
import ResizableLayout from "./components/ResizableLayout";
import ReadDiagnostics from "./components/ReadDiagnostics";
import * as api from "./api";
import { sessionKey } from "./utils";
import { useConversationSearch } from "./useConversationSearch";
import { catalogGroups } from "./sessionGroups";
import { useSessionCatalog, useSessionCounts } from "./useSessionCatalog";
import { catalogFilterFor, DEFAULT_CATALOG_FILTER } from "./catalogCache";
import type { AgentInfo, CatalogFilter, ConfigActivity, ConversationSearchTarget, SessionDeleteFailure, SessionDetail, SessionMeta } from "./types";

const NO_SESSIONS: SessionMeta[] = [];

type Tab = "sessions" | "config" | "usage";

type SessionOperation =
  | { kind: "confirmDelete"; request: SessionDeletion }
  | { kind: "delete"; progress: { completed: number; total: number } | null }
  | { kind: "archive" };

export default function App() {
  const [tab, setTab] = useState<Tab>("sessions");
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [selectedAgent, setSelectedAgent] = useState<string | null>(null);
  // 用量可查看全部 Agent，会话和配置保留当前 Agent。
  const [usageAgent, setUsageAgent] = useState("all");
  const [selectedProject, setSelectedProject] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [readRequest, setReadRequest] = useState<{ session: SessionMeta; pending: boolean; error: string | null } | null>(null);
  const [searchTarget, setSearchTarget] = useState<ConversationSearchTarget | null>(null);
  const [filter, setFilter] = useState(DEFAULT_CATALOG_FILTER);
  const [configRefreshKey, setConfigRefreshKey] = useState(0);
  const [batchMode, setBatchMode] = useState(false);
  const [checkedSessionKeys, setCheckedSessionKeys] = useState<Set<string>>(new Set());
  const [operation, setOperation] = useState<SessionOperation | null>(null);
  const [batchFailures, setBatchFailures] = useState<SessionDeleteFailure[]>([]);
  const mutationRunning = useRef(false);
  const sessionActionsBusy = operation !== null;
  const [toast, setToast] = useState<{ msg: string; error: boolean } | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);
  // 切换会话或离开当前项目后，丢弃尚未返回的正文请求。
  const detailRequestGeneration = useRef(0);
  const closeSessionDetail = useCallback(() => {
    detailRequestGeneration.current++;
    setDetail(null);
    setReadRequest(null);
    setSearchTarget(null);
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
    if (isTauri()) void api.setConfigGuard(configActivity).catch((e) => showToast(`无法设置编辑保护：${String(e)}`, true));
  }, [configActivity, showToast]);

  useEffect(() => {
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (configActivityRef.current.dirty || configActivityRef.current.busy) { event.preventDefault(); event.returnValue = ""; }
    };
    window.addEventListener("beforeunload", beforeUnload);
    const unlisten = isTauri() ? listen("config-exit-requested", () => {
      const activity = configActivityRef.current;
      if (activity.busy) { showToast("配置操作仍在进行，请稍后退出。", true); return; }
      const exit = () => { void api.confirmConfigExit().catch((e) => showToast(String(e), true)); };
      if (activity.dirty) setConfigNavigation({ label: "退出应用", action: exit }); else exit();
    }) : null;
    return () => { window.removeEventListener("beforeunload", beforeUnload); void unlisten?.then((off) => off()); };
  }, [showToast]);

  const protectConfig = (action: () => void, label: string) => {
    const activity = configActivityRef.current;
    if (tab === "config" && activity.busy) { showToast("配置操作仍在进行，请稍后重试。", true); return; }
    if (tab === "config" && activity.dirty) { setConfigNavigation({ label, action }); return; }
    action();
  };

  useEffect(() => {
    api
      .listAgents()
      .then((list) => {
        setAgents(list);
        setSelectedAgent((prev) => {
          const current = prev ? list.find((a) => a.id === prev) : undefined;
          if (current?.installed) return prev;
          const first = list.find((a) => a.installed);
          return first ? first.id : null;
        });
      })
      .catch((e) => showToast(String(e), true));
  }, [showToast]);

  const availableAgents = agents.filter((agent) => agent.installed);
  const currentAgent = availableAgents.find((agent) => agent.id === selectedAgent);
  const supportsArchive = currentAgent?.supportsArchive;
  const catalogFilter = useMemo(() => catalogFilterFor(filter, supportsArchive), [filter, supportsArchive]);
  const { includeArchived } = catalogFilter;
  const onWatchError = useCallback(() => showToast("暂时无法监测会话更新。切回应用时会重试。", true), [showToast]);
  const { cache: catalogCache, catalog, loading, refreshing, error: catalogError, refresh: refreshCatalog } = useSessionCatalog(
    currentAgent, catalogFilter, tab === "sessions", sessionActionsBusy || batchMode, onWatchError,
  );
  const sessionCounts = useSessionCounts(availableAgents, filter, catalogCache, selectedAgent);
  const [requestCounts, setRequestCounts] = useState<ReadonlyMap<string, number | null>>(() => new Map());
  const catalogWarnings = catalog.warnings;
  const batchBusy = loading || refreshing;

  const clearBatchSelection = () => {
    setCheckedSessionKeys(new Set());
    setBatchFailures([]);
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

  const selectAgentNow = (id: string) => {
    if (sessionActionsBusy || mutationRunning.current || id === selectedAgent) return;
    clearBatchSelection();
    setBatchMode(false);
    closeSessionDetail();
    setSelectedAgent(id);
    setSelectedProject(null);
  };

  const selectAgent = (id: string) => {
    if (sessionActionsBusy || mutationRunning.current || id === (tab === "usage" ? usageAgent : selectedAgent)) return;
    const available = availableAgents.some(agent => agent.id === id);
    if (!available && !(tab === "usage" && id === "all")) return;
    protectConfig(() => {
      setUsageAgent(id);
      if (available) selectAgentNow(id);
    }, "切换 Agent");
  };

  const selectProject = (dirName: string | null) => {
    if (sessionActionsBusy || mutationRunning.current) return;
    clearBatchSelection();
    setBatchMode(false);
    closeSessionDetail();
    setSelectedProject(dirName);
  };

  const { groups: sidebarGroups, byGroup, hints: groupHints, projectlessKeys: hiddenPathKeys } = catalogGroups(catalog);
  const scopedSessions = selectedProject === null ? catalog.sessions : byGroup.get(selectedProject) ?? NO_SESSIONS;
  useEffect(() => {
    if (tab !== "sessions" || loading || sessionActionsBusy || selectedProject === null || sidebarGroups.some((group) => group.dirName === selectedProject)) return;
    closeSessionDetail();
    setSelectedProject(null);
    setBatchMode(false);
    setCheckedSessionKeys(new Set());
  }, [sidebarGroups, selectedProject, loading, sessionActionsBusy, tab, closeSessionDetail]);
  const contentSearch = useConversationSearch(selectedAgent, search.trim(), scopedSessions, tab === "sessions" && !loading);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return scopedSessions;
    return scopedSessions.filter((s) => {
      return contentSearch.hits.has(sessionKey(s)) || [s.title, s.sessionId, s.cwd, s.model]
        .filter(Boolean)
        .some((v) => String(v).toLowerCase().includes(q));
    });
  }, [scopedSessions, search, contentSearch.hits]);

  useEffect(() => {
    if (!batchMode || batchBusy || !checkedSessionKeys.size) return;
    const available = new Set(catalog.sessions.map(sessionKey));
    const remaining = [...checkedSessionKeys].filter(key => available.has(key));
    if (remaining.length !== checkedSessionKeys.size) setCheckedSessionKeys(new Set(remaining));
  }, [batchMode, batchBusy, catalog.sessions, checkedSessionKeys]);

  const selectedDetail = useMemo(() => {
    if (tab !== "sessions" || !detail || !selectedAgent) return detail;
    const meta = catalogCache.withCurrentMetadata(selectedAgent, catalogFilter, detail.meta);
    return meta === detail.meta ? detail : { ...detail, meta };
  }, [catalog, catalogCache, catalogFilter, detail, selectedAgent, tab]);

  const selectedSessionKey = detail ? sessionKey(detail.meta) : readRequest ? sessionKey(readRequest.session) : null;

  const changeFilter = (next: CatalogFilter) => {
    if (sessionActionsBusy || mutationRunning.current) return;
    clearBatchSelection();
    setFilter(currentAgent?.supportsArchive ? next : { ...next, includeArchived: filter.includeArchived });
    if (!next.includeArchived && (selectedDetail?.meta ?? readRequest?.session)?.archived) closeSessionDetail();
  };

  const openSession = (s: SessionMeta) => {
    if (!selectedAgent || sessionActionsBusy || batchMode || mutationRunning.current) return;
    void loadSession(selectedAgent, s);
  };

  const loadSession = async (agentId: string, s: SessionMeta) => {
    const generation = ++detailRequestGeneration.current;
    const hit = contentSearch.hits.get(sessionKey(s));
    const target = hit ? { ...hit, query: search.trim() } : null;
    setDetail(null);
    setReadRequest(current => ({
      session: s, pending: true,
      error: current && sessionKey(current.session) === sessionKey(s) ? current.error : null,
    }));
    try {
      const d = await api.readSession(agentId, s.projectDir, s.sessionId);
      if (generation !== detailRequestGeneration.current) return;
      catalogCache.setReadError(agentId, s, null);
      setReadRequest(null);
      setSearchTarget(target);
      setDetail(d);
    } catch (e) {
      if (generation !== detailRequestGeneration.current) return;
      const error = String(e);
      catalogCache.setReadError(agentId, s, error);
      setReadRequest({ session: s, pending: false, error });
    }
  };

  const toggleBatch = () => {
    if (sessionActionsBusy || batchBusy || mutationRunning.current) return;
    closeSessionDetail();
    setBatchMode(!batchMode);
    clearBatchSelection();
  };

  const toggleSession = (session: SessionMeta) => {
    if (sessionActionsBusy || batchBusy) return;
    const key = sessionKey(session);
    setCheckedSessionKeys((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const toggleAll = () => {
    if (sessionActionsBusy || batchBusy) return;
    const allSelected = filtered.every((s) => checkedSessionKeys.has(sessionKey(s)));
    setCheckedSessionKeys(allSelected ? new Set() : new Set(filtered.map(sessionKey)));
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
      await api.setSessionArchived(currentAgent.id, session, archived);
      catalogCache.setArchived(currentAgent.id, session, archived);
      if (selectedSessionKey === sessionKey(session)) {
        if (archived && !includeArchived) closeSessionDetail();
        else void loadSession(currentAgent.id, { ...session, archived });
      }
      showToast(archived ? "会话已归档" : "会话已取消归档");
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
    const targets = new Map(filtered.filter((s) => checkedSessionKeys.has(sessionKey(s))).map((s) => [sessionKey(s), s]));
    requestDeletion({ mode: "batch", sessions: [...targets.values()] });
  };

  const confirmDeletion = async () => {
    if (operation?.kind !== "confirmDelete" || mutationRunning.current) return;
    const { agent, sessions: targets, mode } = operation.request;
    const batch = mode === "batch";
    const total = targets.length;
    mutationRunning.current = true;
    setOperation({ kind: "delete", progress: batch ? { completed: 0, total } : null });
    if (batch) setBatchFailures([]);
    const succeeded: SessionMeta[] = [];
    const failures: SessionDeleteFailure[] = [];
    try {
      for (const session of targets) {
        try {
          await api.deleteSession(agent.id, session.projectDir, session.sessionId);
          succeeded.push(session);
        } catch (error) {
          failures.push({ session, error: String(error) });
        }
        if (batch) setOperation({ kind: "delete", progress: { completed: succeeded.length + failures.length, total } });
      }
      if (succeeded.length) {
        if (succeeded.some((session) => sessionKey(session) === selectedSessionKey)) closeSessionDetail();
        catalogCache.remove(agent.id, succeeded);
      }
      const action = agent.deletePolicy === "trash" ? "移入废纸篓" : "永久删除";
      if (batch) {
        setCheckedSessionKeys(new Set(failures.map(({ session }) => sessionKey(session))));
        setBatchFailures(failures);
        showToast(`已${action} ${succeeded.length} 个会话${failures.length ? `，${failures.length} 个未删除` : ""}`, failures.length > 0);
      } else if (failures.length) {
        showToast(failures[0].error, true);
      } else {
        showToast(`会话已${action}`);
      }
      if (batch || succeeded.length) refreshCatalog();
    } finally {
      setOperation(null);
      mutationRunning.current = false;
    }
  };

  return (
    <div className="app">
      <header className="topbar">
        <AgentSwitcher agents={availableAgents} value={tab === "usage" ? usageAgent : selectedAgent} allowAll={tab === "usage"}
          counts={tab === "usage" ? requestCounts : tab === "sessions" ? sessionCounts.counts : undefined}
          onOpen={tab === "sessions" ? sessionCounts.refresh : undefined}
          disabled={sessionActionsBusy || (tab === "config" && configActivity.busy)} onChange={selectAgent} />
        <nav className="tabs">
          <button className={tab === "sessions" ? "active" : ""} onClick={() => changeTab("sessions")} disabled={sessionActionsBusy}>
            会话
          </button>
          {availableAgents.length > 0 && (
            <button className={tab === "config" ? "active" : ""} onClick={() => changeTab("config")} disabled={sessionActionsBusy}>
              配置
            </button>
          )}
          <button className={tab === "usage" ? "active" : ""} onClick={() => changeTab("usage")} disabled={sessionActionsBusy}>用量</button>
        </nav>
        <div className="topbar-actions">
          {tab !== "usage" && currentAgent?.dataDir && (
            <button
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
            sessions={filtered}
            selectedSessionKey={selectedSessionKey}
            search={search}
            onSearch={changeSearch}
            onSelect={openSession}
            filter={catalogFilter}
            onFilterChange={changeFilter}
            batchMode={batchMode}
            agent={currentAgent}
            onRequestDelete={requestSessionDelete}
            onToggleArchive={toggleSessionArchive}
            toast={showToast}
            checkedSessionKeys={checkedSessionKeys}
            onToggleBatch={toggleBatch}
            onToggleSession={toggleSession}
            onToggleAll={toggleAll}
            onDeleteSelected={requestBatchDelete}
            disabled={sessionActionsBusy}
            loading={loading}
            refreshing={refreshing}
            loadError={catalogError}
            warnings={catalogWarnings}
            onRetryLoad={refreshCatalog}
            progress={operation?.kind === "delete" ? operation.progress : null}
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
          {selectedDetail && currentAgent ? (
            <SessionDetailView
              detail={selectedDetail}
              agent={currentAgent}
              onRequestDelete={requestSessionDelete}
              onToggleArchive={toggleSessionArchive}
              actionsDisabled={sessionActionsBusy}
              toast={showToast}
              searchTarget={searchTarget?.query === search.trim() ? searchTarget : null}
              onRetryRead={() => void openSession(selectedDetail.meta)}
            />
          ) : readRequest ? (
            <div className="detail-read-state" aria-busy={readRequest.pending}>
              {readRequest.error ? <ReadDiagnostics
                className="session-read-error"
                title={/^stage: (parse_json|decode_utf8)$/m.test(readRequest.error) ? "无法解析会话正文" : "无法读取会话正文"}
                errors={[readRequest.error]}
                retry={() => void openSession(readRequest.session)}
                busy={readRequest.pending}
              /> : <p role="status">正在读取会话…</p>}
            </div>
          ) : (
            <div className="placeholder">选择会话查看详情</div>
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
            <div className="placeholder">选择 Agent 查看配置</div>
          )}
        </main>
      ) : <UsagePanel agents={agents} agent={usageAgent} onRequestCounts={setRequestCounts} />}

      {configNavigation && <ConfirmDialog title="放弃未保存的修改？" message={`${configNavigation.label}会丢失未保存的修改。`} confirmLabel="放弃修改" onCancel={() => setConfigNavigation(null)} onConfirm={() => {
        const action = configNavigation.action;
        setConfigNavigation(null); action();
      }} />}

      {operation?.kind === "confirmDelete" && <SessionDeleteDialog
        deletion={operation.request}
        onConfirm={confirmDeletion}
        onCancel={() => setOperation(current => current?.kind === "confirmDelete" ? null : current)}
      />}
      {toast && <div className={`toast ${toast.error ? "error" : ""}`}>{toast.msg}</div>}
    </div>
  );
}
