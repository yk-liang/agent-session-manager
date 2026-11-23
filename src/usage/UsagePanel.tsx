import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { BarChart3, ListFilter, LoaderCircle, Tags } from "lucide-react";
import { useDelayedBusy } from "../useDelayedBusy";
import { useUiPreference } from "../useUiPreference";
import type { PriceStatusFilter, UsageTab } from "../uiPreferences";
import { requestRecordCounts } from "./metrics";
import { summaryQuery, type UsagePeriod } from "./range";
import type { AgentInfo } from "../types";
import type { RequestEvent, UsageSummary } from "./types";
import ModelPrices from "./pricing/ModelPrices";
import RequestEvents from "./RequestEvents";
import UiIcon from "../components/UiIcon";
import Notice from "../components/Notice";
import Segmented from "../components/Segmented";
import UsageAnalysis from "./UsageAnalysis";
import UsageSyncStatus, { formatSyncProgress } from "./UsageSyncStatus";
import { useUsageDate, useUsageSummary, useUsageSync } from "./useUsageData";

const noSummaryRows: UsageSummary["rows"] = [];
const periods: [UsagePeriod, string][] = [
  ["today", "今日"],
  ["week", "近 7 天"],
  ["month", "近 30 天"],
  ["all", "全部"],
];
const subTabs = [
  ["analysis", <><UiIcon icon={BarChart3} />统计分析</>],
  ["events", <><UiIcon icon={ListFilter} />请求记录</>],
  ["prices", <><UiIcon icon={Tags} />模型价格</>],
] as const;

export default function UsagePanel({
  agents,
  agent,
  onRequestCounts,
  onOpenSession,
  showToast,
}: {
  agents: AgentInfo[];
  agent: string;
  onRequestCounts: (counts: ReadonlyMap<string, number | null>) => void;
  onOpenSession: (event: RequestEvent, signal: AbortSignal) => void;
  showToast: (message: string) => void;
}) {
  const [{ tab: subTab, period, priceStatus }, setUsage] = useUiPreference("usage");
  const [pricesVisited, setPricesVisited] = useState(() => subTab === "prices");
  const changePriceStatus = useCallback((priceStatus: PriceStatusFilter) => setUsage(current => ({ ...current, priceStatus })), [setUsage]);
  const pageRef = useRef<HTMLElement>(null);
  const tabScroll = useRef<Partial<Record<UsageTab, number>>>({});
  useLayoutEffect(() => {
    if (pageRef.current) pageRef.current.scrollTop = tabScroll.current[subTab] ?? 0;
  }, [subTab]);
  const switchTab = (tab: UsageTab) => {
    if (tab === subTab) return;
    tabScroll.current[subTab] = pageRef.current?.scrollTop ?? 0;
    if (tab === "prices") setPricesVisited(true);
    setUsage(current => ({ ...current, tab }));
  };
  const now = useUsageDate();
  const { syncStatus, syncUnavailable, pricingRevision, onPricingChanged, checkUsage, recollectUsage } = useUsageSync();
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
          <Segmented label="用量视图" tabs options={subTabs} value={subTab} onChange={switchTab} />
          <div className="usage-filters">
            {subTab !== "prices" && <Segmented label="时间范围" options={periods} value={period}
              onChange={period => setUsage(current => ({ ...current, period }))} />}
            <UsageSyncStatus
              agents={agents}
              synced={synced}
              status={syncStatus}
              unavailable={syncUnavailable}
              onRetry={checkUsage}
              onRecollect={recollectUsage}
            />
          </div>
        </div>

        {queryError && subTab !== "prices" && (
          <Notice
            className="usage-query-error"
            tone="error"
            action={{ label: "重试", onClick: retrySummary }}
          >
            无法读取用量{summary ? "，已保留上次结果" : ""}。
          </Notice>
        )}
        {subTab !== "prices" && unsyncedAgents.length > 0 && (canShowUsage || showImportProgress || !loadingHistory) && (
          <Notice className="usage-coverage" icon={showImportProgress && syncingSelectedHistory ? <UiIcon icon={LoaderCircle} className="usage-spin" /> : undefined}>
            {syncingSelectedHistory
              ? `正在同步 ${pendingHistoryNames} 的历史用量${progressText}。`
              : pendingHistoryAgents.length > 0
                ? `${pendingHistoryNames} 尚未完成首次同步。`
                : "部分 Agent 的首次同步失败。"}
            {canShowUsage && "以下为已同步的用量。"}
          </Notice>
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
            showToast={showToast}
            onOpenSession={onOpenSession}
            syncing={loadingHistory}
          />
        )}
        {pricesVisited && (
          <ModelPrices
            active={subTab === "prices"}
            agent={agent}
            revision={revision}
            statusFilter={priceStatus}
            onStatusFilterChange={changePriceStatus}
            onPricingChanged={onPricingChanged}
          />
        )}
      </div>
    </main>
  );
}
