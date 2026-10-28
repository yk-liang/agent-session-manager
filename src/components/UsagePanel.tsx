import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ChevronDown, LoaderCircle } from "lucide-react";
import * as api from "../api";
import { AutoRefreshScheduler } from "../autoRefresh";
import { observeSessionChanges } from "../sessionWatch";
import { dateKey, formatTokens, formatUsageField, modelTotals, sumTokens, summaryQuery, usageSeries, type UsagePeriod, type UsagePoint } from "../usageView";
import type { AgentInfo, UsageSummary, UsageSyncProgress } from "../types";
import AgentIcon from "./AgentIcon";
import CollectorControl from "./CollectorControl";
import RequestEvents from "./RequestEvents";
import HelpTip from "./HelpTip";
import UiIcon from "./UiIcon";
import { ModelLabel } from "./SessionModel";

const snapshots = new Map<string, UsageSummary>();
const periods: [UsagePeriod, string][] = [["today", "今日"], ["week", "近 7 天"], ["month", "近 30 天"], ["all", "全部"]];

export default function UsagePanel({ agents, refreshKey, onBusyChange }: { agents: AgentInfo[]; refreshKey: number; onBusyChange: (busy: boolean) => void }) {
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [queryError, setQueryError] = useState("");
  const [queryLoading, setQueryLoading] = useState(true);
  const [syncedAt, setSyncedAt] = useState<number | null>(null);
  const [pending, setPending] = useState<Set<string>>(new Set());
  const [errors, setErrors] = useState<Map<string, string>>(new Map());
  const [period, setPeriod] = useState<UsagePeriod>("month");
  const [agent, setAgent] = useState("all");
  const [model, setModel] = useState("all");
  const [expanded, setExpanded] = useState(false);
  const [now, setNow] = useState(() => new Date());
  const [watchError, setWatchError] = useState(false);
  const [eventsRevision, setEventsRevision] = useState(0);
  const [progress, setProgress] = useState<UsageSyncProgress | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const busy = pending.size > 0;
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
      onBusyChange(true); scheduler.setBusy(true); setPending(new Set(sources));
      try {
        // 源依次扫描，界面逐个显示；避免五份大历史同时争用磁盘和 CPU。
        for (const id of sources) {
          if (disposed) break;
          setProgress({ agent: id, phase: "waiting", checkedFiles: 0, totalFiles: null });
          try {
            const synced = await api.syncRequestEvents(id, generation);
            if (!disposed) setSyncedAt(synced.updatedAt);
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
        if (!disposed) { onBusyChange(false); scheduler.setBusy(false); setProgress(null); }
      }
    };
    const scheduler = new AutoRefreshScheduler(() => refresh());
    scheduler.setBusy(true);
    setPending(new Set(sources)); onBusyChange(true);
    setWatchError(false);
    const stop = observeSessionChanges("usage", scheduler, () => { if (!disposed) setWatchError(true); });
    const subscription = listen<{ generation: number; progress: UsageSyncProgress }>("usage-sync-progress", ({ payload }) => {
      if (!disposed && payload.generation === generation) setProgress(payload.progress);
    }).catch(() => null);
    void subscription.then(() => { if (!disposed) void refresh(); });
    return () => {
      disposed = true; stop(); scheduler.dispose(); onBusyChange(false);
      void subscription.then(off => off?.()).catch(() => {});
      void api.cancelUsage().catch(() => {});
    };
  }, [ids, refreshKey, onBusyChange]);

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
  const available = summary !== null && (!busy || allRows.length > 0);
  const scopedHealth = Object.entries(summary?.health ?? {}).filter(([id]) => agent === "all" || id === agent).map(([, h]) => h);
  const partial = scopedHealth.some(h => h.partial) || rows.some(r => r.partial) || !!queryError ||
    agents.some(a => (agent === "all" || a.id === agent) && errors.has(a.id));
  const options = modelTotals(scopedRows);
  const models = modelTotals(rows);
  const series = usageSeries(rows, period, now);
  const value = (number: number) => available ? formatTokens(number) : "—";
  const changeAgent = (id: string) => { setAgent(id); setModel("all"); setExpanded(false); };
  const warnings = scopedHealth.flatMap(h => h.warnings);
  if (watchError) warnings.push("自动监听暂不可用，切回应用或点击刷新仍可检查更新。");
  const activeName = agents.find(a => a.id === progress?.agent)?.displayName;
  const phaseNames = { waiting: "等待同步", discovering: "查找历史日志", reading: "读取历史日志", indexing: "整理请求记录", complete: "同步完成" };
  return <main className="usage-page">
    <div className="usage-content">
      <div className="usage-heading">
        <div className="usage-heading-title"><h1>Token 用量</h1><HelpTip label="Token 统计">
          直接汇总已保存请求的模型和 usage，按请求发生时间分桶；打开用量页、切回应用或刷新时增量同步。删除会话后仍保留已保存用量，缓存与思考细分不重复相加。
          <p>缺失分项显示“未提供”；≥ 表示只记录了部分调用的该分项。缓存和思考明细仅汇总日志有提供的字段。</p>
          {partial && <p>部分历史缺少计量或存在格式差异，当前结果可能不完整。</p>}
          {(agent === "all" || agent === "qoder") && summary?.health.qoder?.partial && <p>Qoder 缓存与输入的关系尚未确认，总量仅计已记录输入和输出。</p>}
          {[...new Set(warnings)].map(w => <p key={w}>{w}</p>)}
          {errors.size > 0 && <p>部分来源同步失败，已保存请求仍可统计；点击顶部刷新重试。</p>}
        </HelpTip>{partial && available && <span className="usage-partial">部分统计</span>}</div>
        <div className="usage-heading-status"><CollectorControl /><div className="usage-update">{busy ? <><UiIcon icon={LoaderCircle} className="usage-spin" />正在同步请求</> : syncedAt != null ? `已同步 ${new Date(syncedAt).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })}` : "本地已保存记录"}</div></div>
      </div>
      {busy && <div className="usage-sync-status" role="status">
        <span><UiIcon icon={LoaderCircle} className="usage-spin" /><strong>{activeName ? `${activeName} · ${phaseNames[progress!.phase]}` : "准备同步历史请求"}</strong>
          {progress?.totalFiles != null && <span>已检查 {formatTokens(progress.checkedFiles)} / {formatTokens(progress.totalFiles)} 个文件</span>}<span>已用时 {elapsed} 秒</span></span>
        <p>{available ? "正在显示已保存的用量，后续记录会在同步完成后补入。" : "正在同步本机历史日志，首次读取或数据较多时可能需要几分钟。"}可以切换页面，已保存的进度会保留。</p>
      </div>}
      {queryError && <div className="usage-query-error" role="alert">用量查询失败：{queryError}<button onClick={() => setEventsRevision(n => n + 1)}>重试查询</button></div>}
      <div className="usage-filters">
        <div className="usage-periods" role="group" aria-label="统计时间">
          {periods.map(([id, label]) => <button key={id} aria-pressed={period === id} className={period === id ? "active" : ""} onClick={() => setPeriod(id)}>{label}</button>)}
        </div>
        <div className="usage-selects">
          <span className="usage-select"><select aria-label="统计 Agent" value={agent} onChange={e => changeAgent(e.target.value)}><option value="all">全部 Agent</option>{agents.map(a => <option key={a.id} value={a.id}>{a.displayName}</option>)}</select><UiIcon icon={ChevronDown} /></span>
          <span className="usage-select"><select aria-label="统计模型" value={model} onChange={e => { setModel(e.target.value); setExpanded(false); }}><option value="all">全部模型</option>{model !== "all" && !options.some(m => m.model === model) && <option value={model}>{model}</option>}{options.map(m => <option key={m.model} value={m.model}>{m.modelName}</option>)}</select><UiIcon icon={ChevronDown} /></span>
        </div>
      </div>
      <section className="usage-cards" aria-label="Token 概览">
        {([["总 Token", value(totals.totalTokens)], ["输入 Token", available ? rows.length ? formatUsageField(totals, "input") : "0" : "—"], ["输出 Token", available ? rows.length ? formatUsageField(totals, "output") : "0" : "—"]] as const).map(([label, display]) => <div className="usage-stat" key={label}><span>{label}</span><strong title={available ? display === "未提供" ? "日志未提供该分项" : display.startsWith("≥") ? "部分记录缺少该分项，显示已记录下限" : `已记录 ${display} token` : undefined}>{display}</strong></div>)}
      </section>
      <section className="usage-section usage-trend"><h2>{period === "today" ? "每小时趋势" : "每日趋势"}<small className="usage-time-basis">按请求发生时间 · 本地时区</small></h2>
        {available ? <UsageChart points={series} hourly={period === "today"} /> : <div className="usage-chart-empty">{queryError ? "暂时无法读取用量，请重试查询" : busy ? "正在同步历史用量…" : queryLoading ? "正在读取已保存用量…" : "此时间范围暂无记录"}</div>}
      </section>
      <RequestEvents agents={agents} period={period} now={now} agent={agent} model={model} revision={eventsRevision} syncing={busy} onAgentChange={changeAgent} onModelChange={setModel} />
      <div className="usage-breakdowns">
        <section className="usage-section"><h2>按 Agent</h2><div className="usage-agent-list">
          {agents.map(a => {
            const agentRows = allRows.filter(r => r.agent === a.id && (model === "all" || r.model === model));
            const total = sumTokens(agentRows);
            return <button key={a.id} className={`usage-agent-row${agent === a.id ? " selected" : ""}`} aria-pressed={agent === a.id} onClick={() => changeAgent(agent === a.id ? "all" : a.id)}>
              <span className="usage-agent-name"><AgentIcon agent={a.id} />{a.displayName}</span>
              <span className="usage-agent-value" title={summary?.health[a.id]?.partial ? "部分统计" : undefined}>{pending.has(a.id) && <UiIcon icon={LoaderCircle} className="usage-spin" />}{summary && (!pending.has(a.id) || agentRows.length > 0) ? formatTokens(total.totalTokens) : "—"}</span>
              {errors.has(a.id) && !pending.has(a.id) && <span className="usage-source-error">同步失败：{errors.get(a.id)}</span>}
            </button>;
          })}
        </div></section>
        <section className="usage-section usage-models"><h2>按模型</h2>
          {models.length > 0 ? <><table><thead><tr><th>模型</th><th>Token</th></tr></thead><tbody>
            {(expanded ? models : models.slice(0, 8)).map(m => <tr key={m.model}><td><ModelLabel id={m.model} name={m.modelName} /></td><td title={m.partial ? "部分统计" : undefined}>{formatTokens(m.totalTokens)}</td></tr>)}
          </tbody></table>{models.length > 8 && <button className="usage-more" onClick={() => setExpanded(!expanded)}>{expanded ? "收起" : `显示全部 ${models.length} 个模型`}<UiIcon icon={ChevronDown} /></button>}</> : <div className="usage-empty">{queryError ? "模型用量暂不可用" : busy ? "等待历史用量同步" : "暂无模型记录"}</div>}
        </section>
      </div>
      <details className="usage-details"><summary>缓存与思考明细<UiIcon icon={ChevronDown} /></summary><div className="usage-detail-grid">
        {([["缓存读取", totals.cacheReadTokens, totals.cacheReadRecorded], ["缓存写入", totals.cacheWriteTokens, totals.cacheWriteRecorded], ["思考", totals.reasoningTokens, totals.reasoningRecorded], ["未分项", totals.unclassifiedTokens, true]] as const).map(([label, count, recorded]) => <div key={label}><span>{label}</span><strong>{available ? recorded ? formatTokens(count) : "未提供" : "—"}</strong></div>)}
      </div></details>
    </div>
  </main>;
}

function UsageChart({ points, hourly }: { points: UsagePoint[]; hourly: boolean }) {
  const max = Math.max(1, ...points.map(p => p.totalTokens));
  const width = 930, left = 56, top = 14, base = 172, height = base - top;
  const step = width / Math.max(1, points.length), barWidth = Math.max(1, step * .7);
  const y = (n: number) => base - n / max * height;
  const ticks = [...new Set((hourly ? [0, 6, 12, 18, points.length - 1] : [0, Math.floor((points.length - 1) / 2), points.length - 1]).filter(i => i >= 0 && i < points.length))];
  return <div className="usage-chart"><svg viewBox="0 0 1000 202" role="img" aria-label={`${points.length} ${hourly ? "小时" : "天"}的输入与输出 Token 趋势`} preserveAspectRatio="none">
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
