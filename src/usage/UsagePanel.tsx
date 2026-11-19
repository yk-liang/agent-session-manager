import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { BarChart3, ListFilter, LoaderCircle, Tags } from "lucide-react";
import { useDelayedBusy } from "../useDelayedBusy";
import { requestRecordCounts } from "./metrics";
import { summaryQuery, type UsagePeriod } from "./range";
import type { AgentInfo } from "../types";
import type { UsageSummary } from "./types";
import ModelPrices, { type PriceStatusFilter } from "./ModelPrices";
import RequestEvents, { defaultRequestSelection, type RequestSelection } from "./RequestEvents";
import UiIcon from "../components/UiIcon";
import UsageAnalysis from "./UsageAnalysis";
import UsageSyncStatus, { formatSyncProgress } from "./UsageSyncStatus";
import { useUsageDate, useUsageSummary, useUsageSync } from "./useUsageData";

type UsageTab = "analysis" | "events" | "prices";

const noSummaryRows: UsageSummary["rows"] = [];
const periods: [UsagePeriod, string][] = [
  ["today", "今日"],
  ["week", "近 7 天"],
  ["month", "近 30 天"],
  ["all", "全部"],
];
const subTabs: { id: UsageTab; label: string; icon: typeof BarChart3 }[] = [
  { id: "analysis", label: "统计分析", icon: BarChart3 },
  { id: "events", label: "请求记录", icon: ListFilter },
  { id: "prices", label: "模型价格", icon: Tags },
];
function UsageNotice({ className, tone, busy = false, children, action }: {
  className: string;
  tone: "info" | "error";
  busy?: boolean;
  children: ReactNode;
  action?: { label: string; onClick: () => void; disabled?: boolean };
}) {
  return (
    <div className={`usage-notice ${tone} ${className}`} role={tone === "error" ? "alert" : "status"}>
      {busy && <UiIcon icon={LoaderCircle} className="usage-spin" />}
      <p>{children}</p>
      {action && <button type="button" disabled={action.disabled} onClick={action.onClick}>{action.label}</button>}
    </div>
  );
}

export default function UsagePanel({
  agents,
  agent,
  onRequestCounts,
}: {
  agents: AgentInfo[];
  agent: string;
  onRequestCounts: (counts: ReadonlyMap<string, number | null>) => void;
}) {
  const [subTab, setSubTab] = useState<UsageTab>("analysis");
  const [pricesVisited, setPricesVisited] = useState(false);
  const pageRef = useRef<HTMLElement>(null);
  const tabScroll = useRef<Partial<Record<UsageTab, number>>>({});
  useLayoutEffect(() => {
    if (pageRef.current) pageRef.current.scrollTop = tabScroll.current[subTab] ?? 0;
  }, [subTab]);
  const switchTab = (tab: UsageTab) => {
    if (tab === subTab) return;
    tabScroll.current[subTab] = pageRef.current?.scrollTop ?? 0;
    if (tab === "prices") setPricesVisited(true);
    setSubTab(tab);
  };
  const [priceStatusFilter, setPriceStatusFilter] = useState<PriceStatusFilter>("all");
  const [requestSelection, setRequestSelection] = useState({ agent, filters: defaultRequestSelection });
  const selectRequests = (filters: RequestSelection) => setRequestSelection({ agent, filters });
  const [period, setPeriod] = useState<UsagePeriod>("month");
  const now = useUsageDate();
  const { syncStatus, syncUnavailable, pricingRevision, onPricingChanged, checkUsage } = useUsageSync();
  const syncRevision = syncStatus?.revision ?? 0;
  const { summary, queryLoading, queryError, retrySummary } = useUsageSummary(summaryQuery(period, now), syncRevision, pricingRevision);

  const agentCounts = useMemo(() => {
    if (!summary) return new Map<string, number | null>();
    const { counts, total } = requestRecordCounts(summary.rows);
    for (const id of summary.syncedAgents) {
      if (!counts.has(id)) counts.set(id, 0);
    }
    if (counts.size || !agents.length) counts.set("all", total);
    return counts;
  }, [summary, agents.length]);
  useLayoutEffect(() => {
    onRequestCounts(agentCounts);
  }, [onRequestCounts, agentCounts]);

  const scopedRows = useMemo(() => {
    const rows = summary?.rows ?? noSummaryRows;
    return agent === "all" ? rows : rows.filter(row => row.agent === agent);
  }, [summary, agent]);
  const selectedAgents = agents.filter(a => agent === "all" || a.id === agent);
  const synced = useMemo(() => new Set(summary?.syncedAgents ?? []), [summary]);
  const unsyncedAgents = selectedAgents.filter(a => !synced.has(a.id));
  const incomplete = summary === null || unsyncedAgents.length > 0;
  const syncingSelectedHistory = unsyncedAgents.some(agent =>
    syncStatus ? syncStatus.syncing.includes(agent.id) : !syncUnavailable
  );
  const loadingHistory = incomplete && (syncingSelectedHistory || queryLoading);
  const canShowUsage = summary !== null && (!incomplete || scopedRows.length > 0);
  const showImportProgress = useDelayedBusy(loadingHistory, 300, 0);
  const pendingHistoryAgents = unsyncedAgents.filter(a => !syncStatus?.failedAgents.includes(a.id));
  const pendingHistoryNames = pendingHistoryAgents.map(a => a.displayName).join("、");
  const progress = syncStatus?.progress;
  const progressText = progress && pendingHistoryAgents.some(agent => agent.id === progress.agent)
    ? `，${formatSyncProgress(progress)}`
    : "";
  const emptyText = queryError
    ? "无法读取用量"
    : loadingHistory
      ? "正在同步历史用量…"
      : incomplete
        ? "首次同步尚未完成"
        : "所选范围暂无记录";

  const revision = `${syncRevision}:${Math.max(pricingRevision, summary?.pricingRevision ?? 0)}`;

  return (
    <main ref={pageRef} className={`usage-page${subTab === "events" ? " fill" : ""}`}>
      <div className="usage-content">
        <div className="usage-toolbar">
          <div className="usage-subtabs" role="tablist" aria-label="用量视图">
            {subTabs.map(({ id, label, icon }) => (
              <button
                key={id}
                type="button"
                role="tab"
                aria-selected={subTab === id}
                onClick={() => switchTab(id)}
              >
                <UiIcon icon={icon} />
                <span>{label}</span>
              </button>
            ))}
          </div>

          <div className="usage-filters">
            {subTab !== "prices" && (
              <div className="usage-periods" role="group" aria-label="时间范围">
                {periods.map(([id, label]) => (
                  <button
                    key={id}
                    type="button"
                    aria-pressed={period === id}
                    onClick={() => setPeriod(id)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            )}
            <UsageSyncStatus
              agents={agents}
              synced={synced}
              status={syncStatus}
              unavailable={syncUnavailable}
              onRetry={checkUsage}
            />
          </div>
        </div>

        {queryError && subTab !== "prices" && (
          <UsageNotice
            className="usage-query-error"
            tone="error"
            action={{ label: "重试", onClick: retrySummary }}
          >
            无法读取用量{summary ? "，已保留上次结果" : ""}。
          </UsageNotice>
        )}
        {subTab !== "prices" && unsyncedAgents.length > 0 && (canShowUsage || showImportProgress || !loadingHistory) && (
          <UsageNotice className="usage-coverage" tone="info" busy={showImportProgress && syncingSelectedHistory}>
            {syncingSelectedHistory
              ? `正在同步 ${pendingHistoryNames} 的历史用量${progressText}。`
              : pendingHistoryAgents.length > 0
                ? `${pendingHistoryNames} 尚未完成首次同步。`
                : "部分 Agent 的首次同步失败。"}
            {canShowUsage && "以下为已同步的用量。"}
          </UsageNotice>
        )}
        {subTab === "analysis" ? (
          <UsageAnalysis
            agents={agents}
            agent={agent}
            period={period}
            now={now}
            summary={summary}
            rows={scopedRows}
            incomplete={incomplete}
            emptyText={emptyText}
            canShowUsage={canShowUsage}
          />
        ) : subTab === "events" && (
          <RequestEvents
            key={agent}
            agents={agents}
            period={period}
            now={now}
            agent={agent}
            revision={revision}
            selection={requestSelection.agent === agent ? requestSelection.filters : defaultRequestSelection}
            onSelectionChange={selectRequests}
            syncing={loadingHistory}
          />
        )}
        {pricesVisited && (
          <ModelPrices
            active={subTab === "prices"}
            agents={agents}
            agent={agent}
            revision={revision}
            statusFilter={priceStatusFilter}
            onStatusFilterChange={setPriceStatusFilter}
            onPricingChanged={onPricingChanged}
          />
        )}
      </div>
    </main>
  );
}
