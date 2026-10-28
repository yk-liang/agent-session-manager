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
import { ModelLabel } from "./SessionModel";

const snapshots = new Map<string, UsageSummary>();
const periods: [UsagePeriod, string][] = [["today", "今日"], ["week", "近 7 天"], ["month", "近 30 天"], ["all", "全部"]];

export default function UsagePanel({ agents, agent }: { agents: AgentInfo[]; agent: string }) {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [queryError, setQueryError] = useState("");
  const [queryLoading, setQueryLoading] = useState(true);
  const [retryKey, setRetryKey] = useState(0);
  // 首轮完成后的 0 也是已知结果，后续同步不能让它退回加载占位。
  const [hasSynced, setHasSynced] = useState(false);
  const [pending, setPending] = useState<Set<string>>(new Set());
  const [errors, setErrors] = useState<Map<string, string>>(new Map());
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
  const busy = pending.size > 0;
  // 大块进度提示结束即收起，避免同步完成后仍保留过时的阶段文字。
  const showSync = useDelayedBusy(busy, 300, 0);
  useEffect(() => {
    if (!busy) return;
    const started = performance.now(); setElapsed(0);
    const timer = setInterval(() => setElapsed(Math.floor((performance.now() - started) / 1000)), 1000);
    return () => clearInterval(timer);
  }, [busy]);

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
    // 最大的 Codex 历史放最后，其余来源可以先显示；界面 Agent 顺序保持不变。
    const sources = ids.split(",").filter(Boolean).sort((a, b) => Number(a === "codex") - Number(b === "codex"));
    if (!sources.length) return;
    let disposed = false;
    const generation = api.nextUsageGeneration();
    const refresh = async () => {
      if (disposed) return;
      scheduler.setBusy(true); setPending(new Set(sources));
      try {
        // 源依次扫描，界面逐个显示；避免五份大历史同时争用磁盘和 CPU。
        for (const id of sources) {
          if (disposed) break;
          setProgress({ agent: id, phase: "waiting", checkedFiles: 0, totalFiles: null });
          try {
            await api.syncRequestEvents(id, generation);
            if (!disposed) setErrors(current => { const next = new Map(current); next.delete(id); return next; });
          } catch (error) {
            if (!disposed) setErrors(current => new Map(current).set(id, String(error)));
          } finally {
            if (!disposed) {
              setPending(current => { const next = new Set(current); next.delete(id); return next; });
              // 每完成一个来源就展示它的已存记录，其他大历史继续同步。
              setEventsRevision(n => n + 1);
            }
          }
        }
      } finally {
        if (!disposed) { scheduler.setBusy(false); setProgress(null); setHasSynced(true); }
      }
    };
    const scheduler = new AutoRefreshScheduler(() => refresh());
    scheduler.setBusy(true);
    setPending(new Set(sources));
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
  const allRows = summary?.rows ?? [];
  const scopedRows = allRows.filter(r => agent === "all" || r.agent === agent);
  const rows = scopedRows.filter(r => model === "all" || r.model === model);
  const totals = sumTokens(rows);
  // 其他 Agent 的记录不能证明当前来源已完成导入，尚未读到的用量不能显示为 0。
  const scopePending = agent === "all" ? busy : pending.has(agent);
  const available = summary !== null && (hasSynced || !scopePending || scopedRows.length > 0);
  const scopedHealth = Object.entries(summary?.health ?? {}).filter(([id]) => agent === "all" || id === agent).map(([, h]) => h);
  const partial = scopedHealth.some(h => h.partial) || rows.some(r => r.partial) || !!queryError ||
    agents.some(a => (agent === "all" || a.id === agent) && errors.has(a.id));
  const options = modelTotals(scopedRows);
  const models = modelTotals(rows);
  const series = usageSeries(rows, period, now);
  const value = (number: number) => available ? formatTokens(number) : "—";
  // 已有记录时安静地补入变化；只有尚无可展示数据的历史导入才显示进度。
  const showImportProgress = showSync && busy && !available;
  const activeName = agents.find(a => a.id === progress?.agent)?.displayName;
  const phaseNames = { waiting: "等待同步", discovering: "查找历史日志", reading: "读取历史日志", indexing: "整理请求记录", complete: "同步完成" };
  return <main className="usage-page">
    <div className="usage-content">
      <div className="usage-heading">
        <div className="usage-heading-title"><h1>Token 用量</h1><HelpTip label="Token 统计">
          按请求时间统计，已保存用量不随会话删除。
          <p>“≥”为已知下限；“未提供”不等于 0。</p>
          {partial && <p>部分记录不完整，统计可能有偏差。</p>}
          {watchError && <p>自动更新暂不可用，切回应用会重试。</p>}
        </HelpTip>{partial && available && <span className="usage-partial">部分统计</span>}</div>
        <div className="usage-heading-status"><CollectorControl /></div>
      </div>
      {showImportProgress && <div className="usage-sync-status" role="status">
        <span><UiIcon icon={LoaderCircle} className="usage-spin" /><strong>{activeName ? `${activeName} · ${phaseNames[progress!.phase]}` : "准备同步历史请求"}</strong>
          {progress?.totalFiles != null && <span>已检查 {formatTokens(progress.checkedFiles)} / {formatTokens(progress.totalFiles)} 个文件</span>}<span>已用时 {elapsed} 秒</span></span>
        <p>首次读取可能较慢，可继续使用其他页面。</p>
      </div>}
      {queryError && <div className="usage-query-error" role="alert">用量查询失败：{queryError}<button onClick={() => setEventsRevision(n => n + 1)}>重试查询</button></div>}
      {errors.size > 0 && <div className="usage-sync-error" role="alert"><span>
        {[...errors].map(([id, error], index) => <span key={id} className="usage-source-error" title={error}>{index > 0 && "、"}{agents.find(a => a.id === id)?.displayName ?? id}</span>)}
        暂时无法更新，已保存记录仍可查看。</span><button disabled={busy} onClick={() => setRetryKey(key => key + 1)}>重试同步</button></div>}
      <div className="usage-filters">
        <div className="usage-periods" role="group" aria-label="统计时间">
          {periods.map(([id, label]) => <button key={id} aria-pressed={period === id} className={period === id ? "active" : ""} onClick={() => setPeriod(id)}>{label}</button>)}
        </div>
        <div className="usage-selects">
          <span className="usage-select"><select aria-label="统计模型" value={model} onChange={e => { setModel(e.target.value); setExpanded(false); }}><option value="all">全部模型</option>{model !== "all" && !options.some(m => m.model === model) && <option value={model}>{model}</option>}{options.map(m => <option key={m.model} value={m.model}>{m.modelName}</option>)}</select><UiIcon icon={ChevronDown} /></span>
        </div>
      </div>
      <section className="usage-cards" aria-label="Token 概览">
        {([["总 Token", value(totals.totalTokens)], ["输入 Token", available ? rows.length ? formatUsageField(totals, "input") : "0" : "—"], ["输出 Token", available ? rows.length ? formatUsageField(totals, "output") : "0" : "—"]] as const).map(([label, display]) => <div className="usage-stat" key={label}><span>{label}</span><strong title={available ? display === "未提供" ? "日志未提供该分项" : display.startsWith("≥") ? "部分记录缺少该分项，显示已记录下限" : `已记录 ${display} token` : undefined}>{display}</strong></div>)}
      </section>
      <section className="usage-section usage-trend"><h2>{period === "today" ? "每小时趋势" : "每日趋势"}<small className="usage-time-basis">按请求发生时间 · 本地时区</small></h2>
        {available ? <UsageChart points={series} hourly={period === "today"} /> : <div className="usage-chart-empty">{queryError ? "暂时无法读取用量，请重试查询" : busy ? "正在同步历史用量…" : queryLoading ? "正在读取已保存用量…" : "此时间范围暂无记录"}</div>}
      </section>
      <RequestEvents agents={agents} period={period} now={now} agent={agent} model={model} revision={eventsRevision} syncing={busy && !available} />
      <div className="usage-breakdowns">
        <section className="usage-section usage-models"><h2>按模型<HelpTip label="模型统计说明">
          输入包含缓存；缓存率 = 缓存读取 ÷ 输入。
          <p>成本仅汇总日志记录的美元金额。“—”为未知，“≥”为已知下限。</p>
        </HelpTip></h2>
          {models.length > 0 ? <><div className="usage-model-table-scroll" tabIndex={0} role="region" aria-label="按模型用量明细"><table>
            <colgroup>{[24, 10, 13, 13, 14, 11, 15].map((width, i) => <col key={i} style={{ width: `${width}%` }} />)}</colgroup>
            <thead><tr>{["模型", "请求", "输入", "输出", "缓存读", "缓存率", "成本 (USD)"].map(label => <th scope="col" key={label}>{label}</th>)}</tr></thead><tbody>
              {(expanded ? models : models.slice(0, 8)).map(m => <tr key={m.model}><th scope="row"><ModelLabel id={m.model} name={m.modelName} /></th>{modelUsageCells(m).map((cell, i) => <td key={i} title={cell.title}>{cell.text}</td>)}</tr>)}
            </tbody><tfoot><tr><th scope="row">合计</th>{modelUsageCells(sumModelUsage(rows)).map((cell, i) => <td key={i} title={cell.title}>{cell.text}</td>)}</tr></tfoot>
          </table></div>{models.length > 8 && <button className="usage-more" onClick={() => setExpanded(!expanded)}>{expanded ? "收起" : `显示全部 ${models.length} 个模型`}<UiIcon icon={ChevronDown} /></button>}</> : <div className="usage-empty">{queryError ? "模型用量暂不可用" : busy ? "等待历史用量同步" : "暂无模型记录"}</div>}
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
