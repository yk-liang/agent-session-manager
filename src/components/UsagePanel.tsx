import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ChevronDown, LoaderCircle } from "lucide-react";
import * as api from "../api";
import { AutoRefreshScheduler } from "../autoRefresh";
import { useDelayedBusy } from "../useDelayedBusy";
import { observeSessionChanges } from "../sessionWatch";
import { dateKey, formatTokens, formatUsageField, modelTotals, modelUsageCells, sumModelUsage, sumTokens, summaryQuery, usageSeries, type UsagePeriod, type UsagePoint } from "../usageView";
import type { AgentInfo, UsageSummary, UsageSyncProgress } from "../types";
import CollectorControl from "./CollectorControl";
import RequestEvents from "./RequestEvents";
import HelpTip from "./HelpTip";
import UiIcon from "./UiIcon";
import { modelText } from "../models";
import ModelLabel from "./ModelLabel";
import { ModelIcon } from "./BrandIcon";
import Dropdown from "./Dropdown";
import ReadDiagnostics from "./ReadDiagnostics";

const snapshots = new Map<string, UsageSummary>();
const periods: [UsagePeriod, string][] = [["today", "今日"], ["week", "近 7 天"], ["month", "近 30 天"], ["all", "全部"]];

export default function UsagePanel({ agents, agent }: { agents: AgentInfo[]; agent: string }) {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [queryError, setQueryError] = useState("");
  const [queryLoading, setQueryLoading] = useState(true);
  const [retryKey, setRetryKey] = useState(0);
  const [syncingAgentIds, setSyncingAgentIds] = useState<Set<string>>(() => new Set(agents.map(a => a.id)));
  const [syncErrors, setSyncErrors] = useState<Map<string, string>>(new Map());
  const [period, setPeriod] = useState<UsagePeriod>("month");
  const [modelFilter, setModelFilter] = useState({ agent, value: "all" });
  // Agent 改变的首帧就移除旧模型条件，避免发出跨来源的过期请求。
  const model = modelFilter.agent === agent ? modelFilter.value : "all";
  const setModel = (value: string) => setModelFilter({ agent, value });
  const [expanded, setExpanded] = useState(false);
  useEffect(() => { setModelFilter({ agent, value: "all" }); setExpanded(false); }, [agent]);
  const [now, setNow] = useState(() => new Date());
  const [watchError, setWatchError] = useState(false);
  const [eventsRevision, setEventsRevision] = useState(0);
  const [progress, setProgress] = useState<UsageSyncProgress | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const syncing = syncingAgentIds.size > 0;
  useEffect(() => {
    if (!syncing) return;
    const started = performance.now(); setElapsed(0);
    const timer = setInterval(() => setElapsed(Math.floor((performance.now() - started) / 1000)), 1000);
    return () => clearInterval(timer);
  }, [syncing]);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const update = () => {
      const current = new Date(); setNow(current);
      const midnight = new Date(current.getFullYear(), current.getMonth(), current.getDate() + 1);
      timer = setTimeout(update, midnight.getTime() - current.getTime() + 50);
    };
    update();
    const focus = () => setNow(new Date()); window.addEventListener("focus", focus);
    return () => { clearTimeout(timer); window.removeEventListener("focus", focus); };
  }, []);
  const ids = agents.map(a => a.id).join(",");
  useEffect(() => {
    // Codex 历史通常较大，先同步其他来源。
    const sources = ids.split(",").filter(Boolean).sort((a, b) => Number(a === "codex") - Number(b === "codex"));
    if (!sources.length) return;
    let disposed = false;
    const generation = api.nextUsageGeneration();
    const refresh = async () => {
      if (disposed) return;
      scheduler.setBusy(true); setSyncingAgentIds(new Set(sources));
      try {
        for (const id of sources) {
          if (disposed) break;
          setProgress({ agent: id, phase: "waiting", checkedFiles: 0, totalFiles: null });
          try {
            await api.syncRequestEvents(id, generation);
            if (!disposed) setSyncErrors(current => { const next = new Map(current); next.delete(id); return next; });
          } catch (error) {
            if (!disposed) setSyncErrors(current => new Map(current).set(id, String(error)));
          } finally {
            if (!disposed) {
              setSyncingAgentIds(current => { const next = new Set(current); next.delete(id); return next; });
              setEventsRevision(n => n + 1);
            }
          }
        }
      } finally {
        if (!disposed) { scheduler.setBusy(false); setProgress(null); }
      }
    };
    const scheduler = new AutoRefreshScheduler(() => refresh());
    scheduler.setBusy(true);
    setSyncingAgentIds(new Set(sources));
    setWatchError(false);
    const stop = observeSessionChanges("usage", scheduler, () => { if (!disposed) setWatchError(true); });
    const subscription = listen<{ generation: number; progress: UsageSyncProgress }>("usage-sync-progress", ({ payload }) => {
      if (!disposed && payload.generation === generation) setProgress(payload.progress);
    }).catch(() => null);
    void subscription.then(() => { if (!disposed) void refresh(); });
    return () => {
      disposed = true; stop(); scheduler.dispose();
      void subscription.then(off => off?.()).catch(() => {});
      void api.cancelUsage().catch(() => {});
    };
  }, [ids, retryKey]);

  const queryKey = JSON.stringify(summaryQuery(period, now));
  useEffect(() => {
    let disposed = false;
    setSummary(snapshots.get(queryKey) ?? null); setQueryError(""); setQueryLoading(true);
    const generation = api.nextSummaryGeneration();
    void api.readUsageSummary(JSON.parse(queryKey), generation).then(value => {
      if (disposed) return;
      const normalized = { ...value, rows: value.rows.map(row => ({ ...row, date: row.bucketStart == null ? row.date : dateKey(new Date(row.bucketStart)) })) };
      snapshots.set(queryKey, normalized);
      while (snapshots.size > 8) snapshots.delete(snapshots.keys().next().value!);
      setSummary(normalized);
    }).catch(error => { if (!disposed) setQueryError(String(error)); })
      .finally(() => { if (!disposed) setQueryLoading(false); });
    return () => { disposed = true; void api.cancelUsageSummary().catch(() => {}); };
  }, [queryKey, eventsRevision]);
  const scopedRows = (summary?.rows ?? []).filter(row => agent === "all" || row.agent === agent);
  const rows = scopedRows.filter(r => model === "all" || r.model === model);
  const totals = sumTokens(rows);
  const selectedAgents = agents.filter(a => agent === "all" || a.id === agent);
  const synced = new Set(summary?.syncedAgents ?? []);
  const unsyncedAgents = selectedAgents.filter(a => !synced.has(a.id));
  // 来自同一快照的同步标记用于区分空记录和未同步记录。
  const incomplete = summary === null || unsyncedAgents.length > 0;
  const syncingSelection = unsyncedAgents.some(a => syncingAgentIds.has(a.id));
  const importing = incomplete && (syncingSelection || queryLoading);
  const canShowUsage = summary !== null && (!incomplete || scopedRows.length > 0);
  const scopedHealth = Object.entries(summary?.health ?? {}).filter(([id]) => agent === "all" || id === agent).map(([, h]) => h);
  const partial = scopedHealth.some(h => h.partial) || rows.some(r => r.partial) || !!queryError ||
    agents.some(a => (agent === "all" || a.id === agent) && syncErrors.has(a.id));
  const allModels = modelTotals(scopedRows);
  const modelIds = allModels.map(row => row.model);
  if (model !== "all" && !modelIds.includes(model)) modelIds.unshift(model);
  const options = [{ id: "all", name: "全部模型" },
    ...modelIds.map(id => ({ id, name: modelText(id), icon: <ModelIcon id={id} /> })),
  ];
  const models = model === "all" ? allModels : modelTotals(rows);
  const series = usageSeries(rows, period, now);
  const value = (number: number) => canShowUsage ? formatTokens(number) : "—";
  const showImportProgress = useDelayedBusy(importing, 300, 0);
  const activeImport = unsyncedAgents.find(a => a.id === progress?.agent);
  const waitingNames = unsyncedAgents.map(a => a.displayName).join("、");
  const coverageTitle = syncingSelection ? `正在统计 · 已完成 ${selectedAgents.length - unsyncedAgents.length} / ${selectedAgents.length} 个 Agent`
    : queryLoading ? "正在汇总用量…" : "统计尚未完整";
  const phaseNames = { waiting: "等待同步", discovering: "查找历史日志", reading: "读取历史日志", indexing: "整理请求记录", complete: "同步完成" };
  return <main className="usage-page">
    <div className="usage-content">
      <div className="usage-heading">
        <div className="usage-heading-title"><h1>Token 用量</h1><HelpTip label="Token 统计">
          按请求时间统计。删除会话后，已保存的用量仍可查看。
          <p>“≥”表示已记录用量的下限。</p>
          {partial && <p>部分用量未记录。</p>}
          {watchError && <p>暂时无法自动更新。切回应用时会重试。</p>}
        </HelpTip>{partial && canShowUsage && !incomplete && <span className="usage-partial">记录不完整</span>}</div>
        <div className="usage-heading-status"><CollectorControl /></div>
      </div>
      {queryError && <ReadDiagnostics className="usage-query-error" title="无法查询用量" errors={[queryError]} retry={() => setEventsRevision(n => n + 1)} retryLabel="重试查询" />}
      {syncErrors.size > 0 && <ReadDiagnostics
        className="usage-sync-error"
        title={`${[...syncErrors.keys()].map(id => agents.find(a => a.id === id)?.displayName ?? id).join("、")} 同步失败`}
        description="已保存的用量仍可查看。"
        errors={[...syncErrors].map(([id, error]) => `${agents.find(a => a.id === id)?.displayName ?? id}：${error}`)}
        retry={() => setRetryKey(key => key + 1)} retryLabel="重试同步" busy={syncing}
      />}
      <div className="usage-filters">
        <div className="usage-periods" role="group" aria-label="时间范围">
          {periods.map(([id, label]) => <button key={id} aria-pressed={period === id} className={period === id ? "active" : ""} onClick={() => setPeriod(id)}>{label}</button>)}
        </div>
        <Dropdown className="usage-select usage-model-select" label="模型" searchLabel="搜索模型" value={model}
          options={options} onChange={id => { setModel(id); setExpanded(false); }} />
      </div>
      {incomplete && (canShowUsage || showImportProgress || (!importing && summary !== null)) && <div className="usage-sync-status usage-coverage" role="status">
        <span>{showImportProgress && <UiIcon icon={LoaderCircle} className="usage-spin" />}<strong>{coverageTitle}</strong>
          {activeImport ? <span>{activeImport.displayName} · {phaseNames[progress!.phase]}</span>
            : syncingSelection ? <span>等待读取 {waitingNames}</span> : !queryLoading && <span>尚未完成：{waitingNames}</span>}
          {activeImport && progress?.totalFiles != null && <span>已检查 {formatTokens(progress.checkedFiles)} / {formatTokens(progress.totalFiles)} 个文件</span>}
          {showImportProgress && syncingSelection && <span>用时 {elapsed} 秒</span>}</span>
        <p>{canShowUsage ? "下方仅显示已同步的用量。" : importing ? "首次同步期间可继续浏览。" : "暂无用量。请重试同步。"}</p>
      </div>}
      <section className="usage-cards" aria-label="Token 概览">
        {([[incomplete ? "已统计 Token" : "总 Token", value(totals.totalTokens)], [incomplete ? "已统计输入 Token" : "输入 Token", canShowUsage ? rows.length ? formatUsageField(totals, "input") : "0" : "—"], [incomplete ? "已统计输出 Token" : "输出 Token", canShowUsage ? rows.length ? formatUsageField(totals, "output") : "0" : "—"]] as const).map(([label, display]) => <div className="usage-stat" key={label}><span>{label}</span><strong title={canShowUsage ? display === "未记录" ? "日志未记录此项" : display.startsWith("≥") ? "部分记录缺少该分项，显示已记录下限" : `已记录 ${display} Token` : undefined}>{display}</strong></div>)}
      </section>
      <section className="usage-section usage-trend"><h2>{period === "today" ? "每小时趋势" : "每日趋势"}<small className="usage-time-basis">{incomplete && "已统计 · "}请求时间 · 本地时区</small></h2>
        {canShowUsage ? <UsageChart points={series} hourly={period === "today"} /> : <div className="usage-chart-empty">{queryError ? "无法读取趋势" : importing ? "正在同步历史用量…" : incomplete ? "用量尚未统计完成" : "所选时段暂无记录"}</div>}
      </section>
      <RequestEvents agents={agents} period={period} now={now} agent={agent} model={model} revision={eventsRevision} syncing={importing} />
      <div className="usage-breakdowns">
        <section className="usage-section usage-models"><h2>按模型<HelpTip label="模型统计">
          输入含缓存，缓存率为缓存读取量占输入量的比例。
          <p>成本以日志记录的美元金额为准。“—”表示未记录，“≥”表示下限。</p>
        </HelpTip></h2>
          {models.length > 0 ? <><div className="usage-model-table-scroll" tabIndex={0} role="region" aria-label="按模型用量明细"><table>
            <colgroup>{[24, 10, 13, 13, 14, 11, 15].map((width, i) => <col key={i} style={{ width: `${width}%` }} />)}</colgroup>
            <thead><tr>{["模型", "请求", "输入", "输出", "缓存读取", "缓存率", "成本 (USD)"].map(label => <th scope="col" key={label}>{label}</th>)}</tr></thead><tbody>
              {(expanded ? models : models.slice(0, 8)).map(m => <tr key={m.model}><th scope="row"><ModelLabel model={m.model} /></th>{modelUsageCells(m).map((cell, i) => <td key={i} title={cell.title}>{cell.text}</td>)}</tr>)}
            </tbody><tfoot><tr><th scope="row">{incomplete ? "已统计小计" : "合计"}</th>{modelUsageCells(sumModelUsage(rows)).map((cell, i) => <td key={i} title={cell.title}>{cell.text}</td>)}</tr></tfoot>
          </table></div>{models.length > 8 && <button className="usage-more" onClick={() => setExpanded(!expanded)}>{expanded ? "收起" : `显示全部 ${models.length} 个模型`}<UiIcon icon={ChevronDown} /></button>}</> : <div className="usage-empty">{queryError ? "无法读取模型用量" : importing ? "等待历史用量同步" : incomplete ? "模型用量尚未统计完成" : "暂无模型用量"}</div>}
        </section>
      </div>
    </div>
  </main>;
}

function UsageChart({ points, hourly }: { points: UsagePoint[]; hourly: boolean }) {
  const container = useRef<HTMLDivElement>(null);
  const [containerWidth, setContainerWidth] = useState(0);
  useLayoutEffect(() => {
    const element = container.current;
    if (!element) return;
    const measure = () => setContainerWidth(element.clientWidth);
    const observer = new ResizeObserver(measure);
    measure();
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const max = Math.max(1, ...points.map(p => p.totalTokens));
  // 直接使用 CSS 像素布局，窗口变化时重排图形，不缩放 SVG 内的文字。
  const left = 56, right = 24, width = Math.max(0, containerWidth - left - right);
  const top = 14, base = 172, height = base - top;
  const step = width / Math.max(1, points.length), barWidth = Math.max(1, step * .7);
  const y = (n: number) => base - n / max * height;
  const ticks = [...new Set((hourly ? [0, 6, 12, 18, points.length - 1] : [0, Math.floor((points.length - 1) / 2), points.length - 1]).filter(i => i >= 0 && i < points.length))];
  return <div className="usage-chart" ref={container}><svg role="img" aria-label={`${points.length} ${hourly ? "小时" : "天"}的输入与输出 Token 趋势`}>
    {[0, .5, 1].map(r => <g key={r}><line x1={left} x2={left + width} y1={y(max * r)} y2={y(max * r)} className="usage-grid-line" /><text x={left - 9} y={y(max * r) + 4} textAnchor="end">{formatTokens(max * r, true)}</text></g>)}
    {points.map((p, i) => {
      const x = left + i * step + (step - barWidth) / 2;
      return <g key={p.key} className="usage-bucket" data-bucket={p.key} data-total={p.totalTokens}>
        <rect x={x} y={y(p.inputTokens)} width={barWidth} height={height * p.inputTokens / max} className="usage-area-input" />
        <rect x={x} y={y(p.inputTokens + p.outputTokens)} width={barWidth} height={height * p.outputTokens / max} className="usage-area-output" />
        {p.unclassifiedTokens > 0 && <rect x={x} y={y(p.totalTokens)} width={barWidth} height={height * p.unclassifiedTokens / max} className="usage-area-unknown" />}
        <rect x={left + i * step} y={0} width={step} height={base} className="usage-chart-hit"><title>{`${p.title}\n${formatTokens(p.totalTokens)} Token\n输入 ${p.totalTokens ? formatUsageField(p, "input") : "0"} · 输出 ${p.totalTokens ? formatUsageField(p, "output") : "0"}`}</title></rect>
      </g>;
    })}
    {ticks.map(i => <text key={i} x={left + (i + .5) * step} y={195} textAnchor="middle">{points[i].label}</text>)}
  </svg><div className="usage-legend"><span><i className="input" />输入</span><span><i className="output" />输出</span>{points.some(p => p.unclassifiedTokens > 0) && <span><i className="unknown" />未分项</span>}</div></div>;
}
