import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FolderOpen, RefreshCw } from "lucide-react";
import UiIcon from "./components/UiIcon";
import { openPath } from "@tauri-apps/plugin-opener";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Sidebar from "./components/Sidebar";
import SessionList from "./components/SessionList";
import SessionDetailView from "./components/SessionDetail";
import ConfigPanel from "./components/ConfigPanel";
import UsagePanel from "./components/UsagePanel";
import ConfirmDialog from "./components/ConfirmDialog";
import ResizableLayout from "./components/ResizableLayout";
import * as api from "./api";
import { sessionKey, shortPath } from "./utils";
import { useConversationSearch } from "./useConversationSearch";
import { groupSessions, sessionGroup, PROJECTLESS_GROUP } from "./sessionGroups";
import { AgentCatalogCache, EMPTY_CATALOG } from "./catalogCache";
import { AutoRefreshScheduler } from "./autoRefresh";
import { observeSessionChanges } from "./sessionWatch";
import type { AgentInfo, Catalog, ConfigActivity, ConversationSearchTarget, Project, SessionDeleteFailure, SessionDetail, SessionMeta } from "./types";

type Tab = "sessions" | "config" | "usage";

export default function App() {
  const [tab, setTab] = useState<Tab>("sessions");
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [catalogCache] = useState(() => new AgentCatalogCache(api.catalog, api.resolveModelNames));
  // 初始为 null：等 agent 列表返回再定默认值，
  // 避免未安装 Qoder 的用户在启动时发出一次必然失败的请求
  const [selectedAgent, setSelectedAgent] = useState<string | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedProject, setSelectedProject] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionMeta[]>([]);
  const [projectlessIds, setProjectlessIds] = useState<ReadonlySet<string>>(new Set());
  const [groupHints, setGroupHints] = useState<ReadonlyMap<string, string | null>>(new Map());
  const [search, setSearch] = useState("");
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [searchTarget, setSearchTarget] = useState<ConversationSearchTarget | null>(null);
  const [showEmpty, setShowEmpty] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);
  const [loading, setLoading] = useState(false);
  const [usageLoading, setUsageLoading] = useState(false);
  const [batchMode, setBatchMode] = useState(false);
  const [checkedKeys, setCheckedKeys] = useState<Set<string>>(new Set());
  const [pendingBatch, setPendingBatch] = useState<{ agent: AgentInfo; sessions: SessionMeta[] } | null>(null);
  const [batchProgress, setBatchProgress] = useState<{ completed: number; total: number } | null>(null);
  const [batchFailures, setBatchFailures] = useState<SessionDeleteFailure[]>([]);
  const [singleDeleting, setSingleDeleting] = useState(false);
  const batchRunning = useRef(false);
  const deleting = batchProgress !== null || singleDeleting;
  const autoRefresh = useRef<AutoRefreshScheduler | null>(null);
  const pauseAutoRefresh = deleting || batchMode || pendingBatch !== null;
  const pauseAutoRefreshRef = useRef(pauseAutoRefresh);
  pauseAutoRefreshRef.current = pauseAutoRefresh;
  const [toast, setToast] = useState<{ msg: string; error: boolean } | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);
  // 详情请求代次：切换 agent/项目时递增，过期响应直接丢弃，
  // 防止旧 agent 的慢请求返回后把详情渲染到新 agent 上
  const detailGen = useRef(0);
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
      if (activity.busy) { showToast("配置操作正在处理中，请完成后再退出。", true); return; }
      const exit = () => { void api.confirmConfigExit().catch((e) => showToast(String(e), true)); };
      if (activity.dirty) setConfigNavigation({ label: "退出应用", action: exit }); else exit();
    }) : null;
    return () => { window.removeEventListener("beforeunload", beforeUnload); void unlisten?.then((off) => off()); };
  }, [showToast]);

  const protectConfig = (action: () => void, label: string) => {
    const activity = configActivityRef.current;
    if (tab === "config" && activity.busy) { showToast("配置操作正在处理中，请稍后再试。", true); return; }
    if (tab === "config" && activity.dirty) { setConfigNavigation({ label, action }); return; }
    action();
  };

  const refresh = () => protectConfig(() => setRefreshKey((key) => key + 1), "刷新配置");

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

  const currentAgent = agents.find((a) => a.id === selectedAgent);
  const supportsModelNames = currentAgent?.supportsModelNames ?? false;
  const supportsFastCatalog = currentAgent?.supportsFastCatalog ?? false;

  const clearBatchSelection = () => {
    setCheckedKeys(new Set());
    setPendingBatch(null);
    setBatchFailures([]);
  };

  const changeTab = (next: Tab) => {
    if (next === tab || deleting || batchRunning.current) return;
    protectConfig(() => { clearBatchSelection(); setBatchMode(false); setTab(next); }, "切换标签页");
  };

  const changeSearch = (next: string) => {
    if (deleting || batchRunning.current) return;
    clearBatchSelection();
    setSearch(next);
  };

  const changeShowEmpty = (next: boolean) => {
    if (deleting || batchRunning.current) return;
    clearBatchSelection();
    const cached = selectedAgent ? catalogCache.peek(selectedAgent, next) : undefined;
    if (cached) applyCatalog(cached);
    setShowEmpty(next);
  };

  const applyCatalog = useCallback((cat: Catalog) => {
    setProjects(cat.projects);
    setSessions(cat.sessions);
    setProjectlessIds(new Set(cat.projectlessSessionIds ?? []));
    setGroupHints(new Map((cat.sessionGroupHints ?? []).map((hint) => [sessionKey(hint), hint.root])));
  }, []);

  // 切回先展示该 Agent 自己的快照；后台刷新与名称补全各自完成，不等待外部 CLI。
  useEffect(() => {
    if (!selectedAgent || tab !== "sessions") {
      setLoading(false);
      return;
    }
    let cancelled = false;
    const cached = catalogCache.peek(selectedAgent, showEmpty);
    if (cached) applyCatalog(cached);
    const scheduler = new AutoRefreshScheduler(() => reload());
    autoRefresh.current = scheduler;
    scheduler.setBlocked(pauseAutoRefreshRef.current);
    const reload = async () => {
      scheduler.setBusy(true);
      setLoading(true);
      setPendingBatch(null);
      let refinement: Promise<unknown> | undefined;
      try {
        const cat = await catalogCache.load(selectedAgent, showEmpty);
        if (!cancelled) {
          applyCatalog(cat);
          const available = new Set(cat.sessions.map(sessionKey));
          setCheckedKeys((keys) => new Set([...keys].filter((key) => available.has(key))));
          if (cat.warnings.length > 0) {
            showToast(`部分数据读取失败（${cat.warnings.length} 处）：${cat.warnings[0]}`, true);
          }
        }
        if (!cancelled && supportsFastCatalog && cat.sessions.some((s) => s.statsPending)) {
          const generation = api.nextRefinementGeneration();
          refinement = catalogCache.refine(selectedAgent, showEmpty, (agent, empty) => api.refineCatalog(agent, empty, generation))
            .then((refined) => {
              if (cancelled || !refined) return;
              if (pauseAutoRefreshRef.current) { scheduler.request("focus"); return; }
              applyCatalog(refined);
              if (refined.warnings.length) showToast(`部分数据读取失败（${refined.warnings.length} 处）：${refined.warnings[0]}`, true);
            })
            .catch(() => { /* 首屏仍可打开；下一次日志变化或刷新会重试摘要。 */ });
        }
        void catalogCache.enrich(selectedAgent, showEmpty, supportsModelNames).then((enriched) => {
          if (cancelled || !enriched) return;
          applyCatalog(enriched);
          setDetail((current) => {
            if (!current) return current;
            const meta = catalogCache.withNames(selectedAgent, current.meta);
            return meta === current.meta ? current : { ...current, meta };
          });
        }).catch(() => { /* 名称补全失败保留原始模型标识，下一次刷新可重试。 */ });
        if (!cancelled) setLoading(false);
        // 首屏已可操作；调度器仍等待统计完成，把这段 I/O 计入自动刷新的时间预算。
        await refinement;
      } catch (e) {
        if (!cancelled) showToast(String(e), true);
      } finally {
        if (!cancelled) setLoading(false);
        scheduler.setBusy(false);
      }
    };
    const stopObserving = observeSessionChanges(selectedAgent, scheduler, () => {
      if (!cancelled) showToast("自动监听暂不可用，切回应用或点击刷新仍可更新。", true);
    });
    void reload();
    return () => {
      cancelled = true;
      if (supportsFastCatalog) void api.cancelCatalogRefinement().catch(() => {});
      scheduler.dispose();
      stopObserving();
      if (autoRefresh.current === scheduler) autoRefresh.current = null;
    };
  }, [selectedAgent, tab, showEmpty, refreshKey, showToast, catalogCache, applyCatalog, supportsModelNames, supportsFastCatalog]);

  useEffect(() => { autoRefresh.current?.setBlocked(pauseAutoRefresh); }, [pauseAutoRefresh]);

  const selectAgentNow = (id: string) => {
    if (deleting || batchRunning.current) return;
    clearBatchSelection();
    setBatchMode(false);
    if (id === selectedAgent) {
      // 重点当前 agent = 手动刷新，不能清空后不重载
      setRefreshKey((k) => k + 1);
      return;
    }
    detailGen.current += 1;
    setSelectedAgent(id);
    setSelectedProject(null);
    setDetail(null);
    // 与 Agent 选择一起切换数据，任何一帧都不能把旧 Agent 的卡片当成新 Agent 操作。
    applyCatalog(catalogCache.peek(id, showEmpty) ?? EMPTY_CATALOG);
  };

  const selectAgent = (id: string) => protectConfig(() => selectAgentNow(id), id === selectedAgent ? "刷新配置" : "切换 Agent");

  const selectProject = (dirName: string | null) => {
    if (deleting || batchRunning.current) return;
    clearBatchSelection();
    setBatchMode(false);
    detailGen.current += 1;
    setSelectedProject(dirName);
    setDetail(null);
  };

  const sidebarGroups = useMemo(() => groupSessions(projects, sessions, projectlessIds, groupHints), [projects, sessions, projectlessIds, groupHints]);
  const scopedSessions = useMemo(() => sessions.filter((session) => selectedProject === null || sessionGroup(session, projectlessIds, groupHints) === selectedProject), [sessions, selectedProject, projectlessIds, groupHints]);
  const hiddenPathKeys = useMemo(() => new Set(sessions.filter((session) => sessionGroup(session, projectlessIds, groupHints) === PROJECTLESS_GROUP).map(sessionKey)), [sessions, projectlessIds, groupHints]);
  useEffect(() => {
    if (tab !== "sessions" || loading || deleting || selectedProject === null || sidebarGroups.some((group) => group.dirName === selectedProject)) return;
    // 归属刷新后分组可能消失，回到全部会话，避免停在没有侧栏入口的空筛选中。
    detailGen.current++;
    setSelectedProject(null);
    setDetail(null);
    setSearchTarget(null);
    setBatchMode(false);
    setCheckedKeys(new Set());
  }, [sidebarGroups, selectedProject, loading, deleting, tab]);
  const contentSearch = useConversationSearch(selectedAgent, search.trim(), scopedSessions, tab === "sessions" && !loading);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return scopedSessions.filter((s) => {
      if (!q) return true;
      return contentSearch.hits.has(sessionKey(s)) || [s.title, s.sessionId, s.cwd, s.modelName, s.model]
        .filter(Boolean)
        .some((v) => String(v).toLowerCase().includes(q));
    });
  }, [scopedSessions, search, contentSearch.hits]);

  const openSession = async (s: SessionMeta) => {
    if (!selectedAgent || deleting || batchMode || batchRunning.current) return;
    const gen = ++detailGen.current;
    const agentId = selectedAgent;
    const hit = contentSearch.hits.get(sessionKey(s));
    const target = hit ? { ...hit, query: search.trim() } : null;
    // 选择变化后旧详情立即不可操作（删除/恢复/分叉按钮随之卸载）
    setDetail(null);
    try {
      const d = await api.readSession(agentId, s.projectDir, s.sessionId);
      if (gen !== detailGen.current) return;
      setSearchTarget(target);
      setDetail({ ...d, meta: catalogCache.withNames(agentId, d.meta) });
      if (d.warnings?.length) showToast("部分历史日志不可用，当前显示可读取内容。", true);
    } catch (e) {
      if (gen !== detailGen.current) return;
      showToast(String(e), true);
    }
  };

  const onDeleted = () => {
    if (selectedAgent && detail) {
      catalogCache.remove(selectedAgent, [detail.meta]);
      applyCatalog(catalogCache.peek(selectedAgent, showEmpty) ?? EMPTY_CATALOG);
    }
    setDetail(null);
    setRefreshKey((k) => k + 1);
  };

  const toggleBatch = () => {
    if (deleting || loading || batchRunning.current) return;
    detailGen.current += 1;
    setDetail(null);
    setBatchMode(!batchMode);
    setCheckedKeys(new Set());
    setBatchFailures([]);
    setPendingBatch(null);
  };

  const toggleSession = (session: SessionMeta) => {
    if (deleting || loading) return;
    const key = sessionKey(session);
    setCheckedKeys((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const toggleAll = () => {
    if (deleting || loading) return;
    const allSelected = filtered.every((s) => checkedKeys.has(sessionKey(s)));
    setCheckedKeys(allSelected ? new Set() : new Set(filtered.map(sessionKey)));
  };

  const requestBatchDelete = () => {
    if (!currentAgent || deleting || loading) return;
    const targets = new Map(filtered.filter((s) => checkedKeys.has(sessionKey(s))).map((s) => [sessionKey(s), s]));
    if (targets.size > 0) setPendingBatch({ agent: currentAgent, sessions: [...targets.values()] });
  };

  const confirmBatchDelete = async () => {
    if (!pendingBatch || deleting || loading || batchRunning.current) return;
    batchRunning.current = true;
    const { agent, sessions: targets } = pendingBatch;
    setPendingBatch(null);
    setBatchFailures([]);
    setBatchProgress({ completed: 0, total: targets.length });
    detailGen.current += 1;
    setDetail(null);
    const succeeded: SessionMeta[] = [];
    const failures: SessionDeleteFailure[] = [];
    for (const session of targets) {
      try {
        await api.deleteSession(agent.id, session.projectDir, session.sessionId);
        succeeded.push(session);
      } catch (error) {
        failures.push({ session, error: String(error) });
      }
      setBatchProgress({ completed: succeeded.length + failures.length, total: targets.length });
    }
    catalogCache.remove(agent.id, succeeded);
    applyCatalog(catalogCache.peek(agent.id, showEmpty) ?? EMPTY_CATALOG);
    setCheckedKeys(new Set(failures.map(({ session }) => sessionKey(session))));
    setBatchFailures(failures);
    const action = agent.deletePolicy === "trash" ? "移入回收站" : "永久删除";
    showToast(`已${action} ${succeeded.length} 个会话${failures.length ? `，${failures.length} 个失败，可重试` : ""}`, failures.length > 0);
    setLoading(true);
    setRefreshKey((k) => k + 1);
    setBatchProgress(null);
    batchRunning.current = false;
  };

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="brand-dot" />
          Agent Session Manager
        </div>
        <nav className="tabs">
          <button className={tab === "sessions" ? "active" : ""} onClick={() => changeTab("sessions")} disabled={deleting}>
            会话
          </button>
          {agents.some((a) => a.installed) && (
            <button className={tab === "config" ? "active" : ""} onClick={() => changeTab("config")} disabled={deleting}>
              配置
            </button>
          )}
          <button className={tab === "usage" ? "active" : ""} onClick={() => changeTab("usage")} disabled={deleting}>用量</button>
        </nav>
        <div className="topbar-actions">
          <button className="button-with-icon" onClick={refresh} disabled={loading || deleting || tab === "usage" && usageLoading}>
            <UiIcon icon={RefreshCw} />
            {loading || tab === "usage" && usageLoading ? "刷新中…" : "刷新"}
          </button>
          {tab !== "usage" && currentAgent?.dataDir && (
            <button
              className="button-with-icon"
              onClick={() =>
                openPath(currentAgent.dataDir!).catch((e) => showToast(`打开数据目录失败: ${e}`, true))
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
            agents={agents.filter((a) => a.installed)}
            selectedAgent={selectedAgent}
            onSelectAgent={selectAgent}
            projects={sidebarGroups}
            selected={selectedProject}
            onSelect={selectProject}
            disabled={deleting}
          />}
          sessionList={<SessionList
            sessions={filtered}
            selectedKey={detail ? sessionKey(detail.meta) : null}
            search={search}
            onSearch={changeSearch}
            onSelect={openSession}
            showEmpty={showEmpty}
            onShowEmpty={changeShowEmpty}
            batchMode={batchMode}
            checkedKeys={checkedKeys}
            onToggleBatch={toggleBatch}
            onToggleSession={toggleSession}
            onToggleAll={toggleAll}
            onDeleteSelected={requestBatchDelete}
            deleting={deleting}
            loading={loading}
            progress={batchProgress}
            failures={batchFailures}
            searchHits={contentSearch.hits}
            searchPending={contentSearch.pending}
            searchSkipped={contentSearch.skipped}
            searchError={contentSearch.error}
            showPaths={selectedProject === null}
            hiddenPathSessionKeys={hiddenPathKeys}
            displayRoots={groupHints}
          />}
        >
          {detail && currentAgent ? (
            <SessionDetailView
              detail={detail}
              agent={currentAgent}
              onDeleted={onDeleted}
              onDeletingChange={setSingleDeleting}
              toast={showToast}
              searchTarget={searchTarget?.query === search.trim() ? searchTarget : null}
            />
          ) : (
            <div className="placeholder">选择一个会话查看详情</div>
          )}
        </ResizableLayout>
      ) : tab === "config" ? (
        <ResizableLayout
          sidebar={<Sidebar
            agents={agents.filter((a) => a.installed)}
            selectedAgent={selectedAgent}
            onSelectAgent={selectAgent}
            projects={sidebarGroups}
            selected={selectedProject}
            onSelect={selectProject}
            disabled={deleting}
            hideProjects
          />}
        >
          {selectedAgent ? (
            <ConfigPanel
              key={selectedAgent}
              agent={selectedAgent}
              displayName={currentAgent?.displayName ?? selectedAgent}
              refreshKey={refreshKey}
              toast={showToast}
              onActivityChange={onConfigActivity}
            />
          ) : (
            <div className="placeholder">选择一个 Agent 查看配置</div>
          )}
        </ResizableLayout>
      ) : <UsagePanel agents={agents} refreshKey={refreshKey} onBusyChange={setUsageLoading} />}

      {configNavigation && <ConfirmDialog title="放弃未保存修改？" message={`继续${configNavigation.label}会放弃尚未写入文件的草稿。你也可以取消，返回编辑器保存。`} confirmLabel="放弃修改并继续" onCancel={() => setConfigNavigation(null)} onConfirm={() => {
        const action = configNavigation.action;
        setConfigNavigation(null); action();
      }} />}

      {pendingBatch && (
        <ConfirmDialog
          title="批量删除会话"
          message={
            <>
              <p>确定删除 {pendingBatch.agent.displayName} 的 {pendingBatch.sessions.length} 个会话吗？</p>
              <p>{pendingBatch.agent.deletePolicy === "trash"
                ? "会话文件将移入系统回收站，可以手动还原。"
                : "会话将永久删除，无法恢复。"}</p>
              {pendingBatch.agent.id === "opencode" && <p>OpenCode CLI 也会删除关联的子会话。</p>}
              <ul className="batch-targets">
                {pendingBatch.sessions.map((session) => (
                  <li key={sessionKey(session)}>
                    {session.title || session.sessionId}
                    <small>{shortPath(session.cwd) || session.projectDir} · {session.sessionId}</small>
                  </li>
                ))}
              </ul>
              <p>逐条执行；失败项会列出，已完成的删除不会自动回滚。</p>
            </>
          }
          confirmLabel={`${pendingBatch.agent.deletePolicy === "trash" ? "移入回收站" : "永久删除"}（${pendingBatch.sessions.length}）`}
          onConfirm={confirmBatchDelete}
          onCancel={() => setPendingBatch(null)}
        />
      )}
      {toast && <div className={`toast ${toast.error ? "error" : ""}`}>{toast.msg}</div>}
    </div>
  );
}
