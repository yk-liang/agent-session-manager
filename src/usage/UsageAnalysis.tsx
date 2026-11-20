import Segmented from "../components/Segmented";
import { useCallback, useMemo, useState, type ReactNode } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import type { AgentInfo } from "../types";
import type { UsageSummary, UsageSummaryRow } from "./types";
import { display, formatTokens, formatCount, formatPercent, type Display } from "./format";
import { cacheCoverageNote, cacheRateDisplay, costDisplay, costGapNotes, dimensionTotals, distributionSlices, groupUsageBy, groupUsageCells, OTHERS_KEY, requestDisplay, successRateDisplay, sumModelUsage, tokenDisplay, uncachedInputOf, type AnalysisDimension, type GroupedUsage, type ModelUsage, type TrendMetric } from "./metrics";
import { buildTrendSeries, entityColors, OTHER_COLOR, TREND_METRIC_NAMES, usageSeries, type TrendGroup } from "./trend";
import { type UsagePeriod } from "./range";
import { projectName, shortPath } from "../utils";
import AgentIcon from "../components/AgentIcon";
import { ProviderIcon } from "../components/BrandIcon";
import ChartLegend from "./ChartLegend";
import { anchorOf, useTooltip } from "./UsageTooltip";
import ModelLabel from "../components/ModelLabel";
import UiIcon from "../components/UiIcon";
import UsageNumber from "./UsageNumber";
import UsageTrendChart from "./UsageTrendChart";

interface Props {
  agents: AgentInfo[];
  agent: string;
  period: UsagePeriod;
  now: Date;
  summary: UsageSummary | null;
  rows: UsageSummaryRow[];
  incomplete: boolean;
  emptyText: string;
  canShowUsage: boolean;
}

const TREND_METRIC_OPTIONS = Object.entries(TREND_METRIC_NAMES) as [TrendMetric, string][];
const SHARE_METRIC_OPTIONS = TREND_METRIC_OPTIONS.map(([id, name]): [TrendMetric, string] => [id, `${name}占比`]);
const DIMENSIONS: Record<AnalysisDimension, string> = { models: "模型", agents: "Agent", projects: "项目", providers: "提供商" };
const TABLE_ROWS = 8;

function highestCost(rows: GroupedUsage[]): GroupedUsage | null {
  return rows.reduce<GroupedUsage | null>((leader, row) =>
    row.key && row.costUsd != null && (leader?.costUsd == null || row.costUsd > leader.costUsd)
      ? row
      : leader,
  null);
}

function CostLeader({ leader, label }: { leader: GroupedUsage | null; label?: (key: string) => string }) {
  if (!leader || leader.costUsd == null) return "—";
  return (
    <>
      <span className="usage-stat-value-detail" title={leader.key}>{label ? label(leader.key) : leader.key}</span>
      <UsageNumber value={costDisplay(leader)} />
    </>
  );
}

function GroupUsageCells({ row }: { row: ModelUsage }) {
  return <>
    <td>
      <UsageNumber value={requestDisplay(row)} />
      {row.failedCount > 0 && <> / <span className="usage-failed-count" title="失败请求">{formatCount(row.failedCount)}</span></>}
    </td>
    <td>{successRateDisplay(row)}</td>
    {groupUsageCells(row).map((cell, i) => <td key={i}><UsageNumber value={cell} /></td>)}
  </>;
}

function StatCard({ metric, label, value, rawValue, meta, notes }: {
  metric: TrendMetric | "cache";
  label: string;
  value: Display;
  rawValue?: number;
  meta: [string, ReactNode][];
  notes?: string[];
}) {
  return (
    <div className="usage-stat" data-metric={metric}>
      <span>{label}</span>
      <strong data-value={rawValue}><UsageNumber value={value} /></strong>
      <div className="usage-stat-meta">
        {meta.map(([name, content]) => (
          <div key={name}>
            <span>{name}</span>
            <b className="usage-stat-value">{content}</b>
          </div>
        ))}
      </div>
      {notes && notes.length > 0 && (
        <ul className="usage-stat-notes">
          {notes.map(note => <li key={note}>{note}</li>)}
        </ul>
      )}
    </div>
  );
}

export default function UsageAnalysis({
  agents, agent, period, now, summary, rows, incomplete, emptyText, canShowUsage,
}: Props) {
  const [trendMetric, setTrendMetric] = useState<TrendMetric>("tokens");
  const [trendGroup, setTrendGroup] = useState<TrendGroup>("category");
  const [dimension, setDimension] = useState<AnalysisDimension>("models");
  const [shareMetric, setShareMetric] = useState<TrendMetric>("tokens");
  const [expanded, setExpanded] = useState(false);
  const shareTip = useTooltip();
  const agentNames = useMemo(() => new Map(agents.map(item => [item.id, item.displayName])), [agents]);

  const group: TrendGroup = agent !== "all" && trendGroup === "agents" ? "category" : trendGroup;
  const groupBy: AnalysisDimension = agent !== "all" && dimension === "agents" ? "models" : dimension;

  const totals = useMemo(() => sumModelUsage(rows), [rows]);
  const models = useMemo(() => groupUsageBy(rows, "models"), [rows]);
  const projects = useMemo(() => dimensionTotals(summary?.projects, agent), [summary?.projects, agent]);
  const points = useMemo(() => usageSeries(rows, period, now), [rows, period, now]);
  const trendLabel = useCallback(
    (kind: "models" | "agents", key: string) => kind === "agents" ? agentNames.get(key) ?? key : key,
    [agentNames],
  );

  const groupedRows = useMemo<GroupedUsage[]>(() => {
    if (groupBy === "models") return models;
    if (groupBy === "agents") return groupUsageBy(rows, "agents");
    if (groupBy === "projects") return projects;
    return dimensionTotals(summary?.providers, agent);
  }, [groupBy, rows, models, projects, summary?.providers, agent]);
  // 颜色跟随实体（按总 Token 排序固定），趋势与占比使用同一映射。
  const colors = useMemo(() => entityColors(groupedRows.map(row => row.key)), [groupedRows]);
  const trendColors = useMemo(
    () => (group === "category" ? undefined : group === groupBy ? colors : entityColors(groupUsageBy(rows, group).map(g => g.key))),
    [group, groupBy, colors, rows],
  );
  const series = useMemo(
    () => buildTrendSeries(rows, points, period, trendMetric, group, trendLabel, trendColors),
    [rows, points, period, trendMetric, group, trendLabel, trendColors],
  );
  const slices = useMemo(() => distributionSlices(groupedRows, shareMetric, 6, groupBy === "agents"), [groupedRows, shareMetric, groupBy]);
  const footer = useMemo(() => sumModelUsage(groupedRows), [groupedRows]);

  const sliceLabel = (key: string, othersCount?: number) => {
    if (othersCount) return `其他 ${othersCount} 项`;
    if (groupBy === "models") return key;
    if (groupBy === "agents") return agentNames.get(key) ?? key;
    if (groupBy === "projects") return projectName(key);
    return key || "提供商未记录";
  };
  const nameCell = (key: string) => {
    if (groupBy === "models") return <ModelLabel model={key} />;
    if (groupBy === "agents") {
      return <span className="usage-dim-agent"><AgentIcon agent={key} inline /><span>{agentNames.get(key) ?? key}</span></span>;
    }
    if (groupBy === "projects") {
      return (
        <span className="usage-dim-project" title={key || "项目未记录"}>
          <span>{projectName(key)}</span>
          {key && <small>{shortPath(key)}</small>}
        </span>
      );
    }
    return (
      <span className="usage-dim-provider" title={key || "提供商未记录"}>
        {key ? <ProviderIcon provider={key} /> : null}
        <span>{key || "提供商未记录"}</span>
      </span>
    );
  };

  const hasRows = canShowUsage && rows.length > 0;
  const shown = (value: Display) => (canShowUsage ? value : display(null, { missingTitle: emptyText }));
  const zeroOr = (value: Display) => (hasRows ? value : canShowUsage ? display(0) : shown(value));
  const untimed = Object.entries(summary?.untimed ?? {})
    .filter(([id]) => agent === "all" || id === agent)
    .reduce((sum, [, count]) => sum + count, 0);
  const tokenNotes = [
    (totals.inputIncomplete || totals.outputIncomplete) && "部分请求缺少输入或输出 Token，合计仅包含已知用量。",
    period !== "all" && untimed > 0 && `${formatCount(untimed)} 条记录未记录时间，可在「全部」中查看。`,
  ];
  const cumulativeRecords = Math.max(0, totals.recordCount - totals.requestCount);
  const requestNotes = [totals.requestCountIncomplete && cumulativeRecords > 0 && `${formatCount(cumulativeRecords)} 条累计用量记录未计入请求次数`];
  const cacheRate = cacheRateDisplay(totals);
  const cacheNotes = [
    cacheRate.missing && totals.inputTokens > 0 && "缓存读取数据不完整，无法计算命中率。",
    cacheCoverageNote(totals),
  ];
  const cost = costDisplay(totals);
  const clean = (notes: (string | false | undefined)[]) => (hasRows ? notes.filter((n): n is string => !!n) : []);

  const completedRequests = totals.successCount + totals.failedCount;
  const cacheReads = shown(tokenDisplay(totals, "cacheRead", { incomplete: totals.cacheReadIncomplete }));
  const hasCacheWrites = totals.cacheWriteTokens > 0;
  const uncached = uncachedInputOf(totals);
  const topModel = hasRows ? highestCost(models.filter(row => row.key !== "unknown")) : null;
  const topProject = hasRows ? highestCost(projects) : null;

  return (
    <>
      <section className="usage-cards" aria-label="用量概览">
        <StatCard
          metric="requests"
          label={incomplete ? "已统计请求" : "请求总数"}
          value={zeroOr(requestDisplay(totals))}
          meta={[
            ["成功 / 失败", hasRows && completedRequests > 0
              ? `${formatCount(totals.successCount)} / ${formatCount(totals.failedCount)}`
              : "—"],
            ["成功率", hasRows ? successRateDisplay(totals) : "—"],
          ]}
          notes={clean(requestNotes)}
        />
        <StatCard
          metric="tokens"
          label={incomplete ? "已统计 Token" : "Token 用量"}
          value={zeroOr(display(totals.totalTokens, {
            compact: true,
            exactness: totals.inputIncomplete || totals.outputIncomplete ? "lowerBound" : "exact",
            unit: "Token",
          }))}
          rawValue={canShowUsage ? totals.totalTokens : undefined}
          meta={[
            ["输入", <UsageNumber value={zeroOr(tokenDisplay(totals, "input"))} />],
            ["输出", <UsageNumber value={zeroOr(tokenDisplay(totals, "output"))} />],
          ]}
          notes={clean(tokenNotes)}
        />
        <StatCard
          metric="cache"
          label="缓存命中率"
          value={shown(cacheRate)}
          meta={[
            [hasCacheWrites ? "缓存读取 / 写入" : "缓存读取", <>
              <UsageNumber value={cacheReads} />
              {hasCacheWrites && <> / <UsageNumber value={shown(tokenDisplay(totals, "cacheWrite"))} /></>}
            </>],
            ["未缓存输入", <UsageNumber value={shown(display(uncached, { compact: true, unit: "Token", missingTitle: "缓存读写数据不完整，无法计算未缓存输入" }))} />],
          ]}
          notes={clean(cacheNotes)}
        />
        <StatCard
          metric="cost"
          label={incomplete ? "已统计费用" : "总费用"}
          value={hasRows ? cost : canShowUsage ? display(0, { kind: "usd" }) : shown(cost)}
          meta={[
            ["模型第一", <CostLeader leader={topModel} />],
            ["项目第一", <CostLeader leader={topProject} label={projectName} />],
          ]}
          notes={costGapNotes(totals)}
        />
      </section>

      <section className="usage-section usage-trend" aria-label="用量趋势">
        <div className="usage-trend-heading">
          <div className="usage-section-title">
            <h2>{period === "today" ? "每小时趋势" : "每日趋势"}</h2>
          </div>
          <div className="usage-controls">
            <Segmented compact label="趋势指标" options={TREND_METRIC_OPTIONS} value={trendMetric} onChange={setTrendMetric} />
            <Segmented<TrendGroup> compact
              label="趋势分组"
              options={agent === "all" ? [["category", "按类别"], ["models", "按模型"], ["agents", "按 Agent"]] : [["category", "按类别"], ["models", "按模型"]]}
              value={group}
              onChange={setTrendGroup}
            />
          </div>
        </div>
        {canShowUsage ? (
          <UsageTrendChart key={`${period}:${trendMetric}:${group}`} points={points} series={series} metric={trendMetric} hourly={period === "today"} />
        ) : (
          <div className="usage-chart-empty">{emptyText}</div>
        )}
      </section>

      <section className="usage-section usage-models" aria-label="用量分组明细">
        <div className="usage-trend-heading">
          <div className="usage-section-title">
            <h2>按{DIMENSIONS[groupBy]}</h2>
          </div>
          <div className="usage-controls">
            <Segmented compact
              label="分组维度"
              options={(Object.entries(DIMENSIONS) as [AnalysisDimension, string][])
                .filter(([id]) => agent === "all" || id !== "agents")
                .map(([id, text]): [AnalysisDimension, string] => [id, `按${text === "Agent" ? " Agent" : text}`])}
              value={groupBy}
              onChange={id => { setDimension(id); setExpanded(false); }}
            />
            <Segmented compact
              label="占比指标"
              options={SHARE_METRIC_OPTIONS}
              value={shareMetric}
              onChange={setShareMetric}
            />
          </div>
        </div>

        {slices.some(slice => slice.value > 0) && (
          <div className="usage-distribution">
            <div className="usage-dist-bar" role="img" aria-label={`${DIMENSIONS[groupBy]}${TREND_METRIC_NAMES[shareMetric]}占比`}>
              {slices.map(slice => {
                const color = slice.othersCount ? OTHER_COLOR : colors.get(slice.key) ?? OTHER_COLOR;
                const label = sliceLabel(slice.key, slice.othersCount);
                return (
                  <span
                    key={slice.key}
                    style={{ width: `${Math.max(slice.share, 0.5)}%`, background: color }}
                    onPointerEnter={event => shareTip.show({
                      title: label,
                      rows: [
                        { key: "share", label: "占比", value: formatPercent(slice.share), color },
                        { key: "tokens", label: "Token", value: formatTokens(slice.totalTokens) },
                        { key: "requests", label: "请求", value: requestDisplay(slice).text },
                        { key: "cost", label: "费用", value: costDisplay(slice).text },
                      ],
                      note: slice.othersCount ? `合并了占比较小的 ${slice.othersCount} 项` : undefined,
                    }, anchorOf(event.currentTarget))}
                    onPointerLeave={shareTip.hide}
                  />
                );
              })}
            </div>
            {shareTip.tooltip}
            <ChartLegend
              label="占比图例"
              items={slices.map(slice => ({
                key: slice.key,
                label: sliceLabel(slice.key, slice.othersCount),
                color: slice.othersCount ? OTHER_COLOR : colors.get(slice.key) ?? OTHER_COLOR,
                detail: formatPercent(slice.share),
              }))}
            />
          </div>
        )}

        {groupedRows.length > 0 ? (
          <>
            <div className="usage-model-table-scroll" tabIndex={0} role="region" aria-label={`${DIMENSIONS[groupBy]}用量明细`}>
              <table>
                <colgroup>
                  {[24, 12, 10, 10, 10, 11, 10, 13].map((width, i) => <col key={i} style={{ width: `${width}%` }} />)}
                </colgroup>
                <thead>
                  <tr>
                    {[DIMENSIONS[groupBy], "请求", "成功率", "输入", "输出", "缓存读取", "缓存命中率", "费用（USD）"].map(label => (
                      <th scope="col" key={label}>{label}</th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {(expanded ? groupedRows : groupedRows.slice(0, TABLE_ROWS)).map(row => (
                    <tr key={row.key || OTHERS_KEY}>
                      <th scope="row">{nameCell(row.key)}</th>
                      <GroupUsageCells row={row} />
                    </tr>
                  ))}
                </tbody>
                <tfoot>
                  <tr>
                    <th scope="row">{incomplete ? "已同步小计" : "合计"}</th>
                    <GroupUsageCells row={footer} />
                  </tr>
                </tfoot>
              </table>
            </div>
            {groupedRows.length > TABLE_ROWS && (
              <button type="button" className="usage-more btn-ghost btn-accent" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
                {expanded ? "收起" : `显示全部 ${formatCount(groupedRows.length)} 项`}
                <UiIcon icon={expanded ? ChevronUp : ChevronDown} />
              </button>
            )}
          </>
        ) : (
          <div className="usage-empty">{canShowUsage ? "所选范围暂无记录" : emptyText}</div>
        )}
      </section>
    </>
  );
}
