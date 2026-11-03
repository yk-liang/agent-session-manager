import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ChevronDown, ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, LoaderCircle } from "lucide-react";
import CopyButton from "./CopyButton";
import { useDelayedBusy } from "../useDelayedBusy";
import type { UsageModelLabel, UsageModelRef } from "../useUsageModelNames";
import * as api from "../api";
import type { AgentInfo, EventPage, EventQuery, RequestEvent } from "../types";
import { dateKey, firstDate, formatTokens, type UsagePeriod } from "../usageView";
import { shortPath } from "../utils";
import HelpTip from "./HelpTip";
import UiIcon from "./UiIcon";
import { ModelLabel } from "./SessionModel";

interface Props {
  agents: AgentInfo[];
  period: UsagePeriod;
  now: Date;
  agent: string;
  model: string;
  revision: number;
  syncing?: boolean;
  modelLabel: UsageModelLabel;
  onModels: (models: UsageModelRef[]) => void;
}
const resultNames = { success: "成功", failed: "失败", unknown: "未记录" };
const duration = (ms: number | null) => ms == null ? "—" : ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(2)} s`;
const timingNames = { recorded: "日志记录耗时", observed: "请求与响应日志的时间差", estimated: "按相邻事件估算，可能包含调度、重试及工具等待" };
interface ViewAnchor { id: string; detail: boolean; offset: number; scrollTop: number }

function visibleRequest(table: HTMLDivElement | null): ViewAnchor | null {
  if (!table) return null;
  const top = table.getBoundingClientRect().top + (table.querySelector("thead")?.getBoundingClientRect().height ?? 0);
  const row = [...table.querySelectorAll<HTMLTableRowElement>("tr[data-event-id]")].find(row => row.getBoundingClientRect().bottom > top);
  return row ? { id: row.dataset.eventId!, detail: row.classList.contains("request-detail-row"), offset: row.getBoundingClientRect().top - top, scrollTop: table.scrollTop } : null;
}

export default function RequestEvents({ agents, period, now, agent, model, revision, syncing = false, modelLabel, onModels }: Props) {
  const [status, setStatus] = useState("all");
  const [page, setPage] = useState<EventPage | null>(null);
  const [pageSize, setPageSize] = useState(50);
  const [jump, setJump] = useState("1");
  const [queuedSync, setQueuedSync] = useState(0);
  const [loading, setLoading] = useState(false);
  const [loadingFeedback, setLoadingFeedback] = useState(false);
  const navigating = loading && loadingFeedback;
  const showLoading = useDelayedBusy(navigating);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const serial = useRef(0);
  const currentPage = useRef<EventPage | null>(null);
  const busy = useRef(false);
  const latestRevision = useRef(revision);
  latestRevision.current = revision;
  const table = useRef<HTMLDivElement>(null);
  const jumpInput = useRef<HTMLInputElement>(null);
  const restoreView = useRef<ViewAnchor | "top" | null>(null);
  const retry = useRef({ page: 1, refresh: false });
  const from = firstDate(period, now);
  const until = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1).getTime();
  const query: EventQuery = {
    ...(agent !== "all" && { agent }), ...(model !== "all" && { model }), ...(status !== "all" && { status }),
    ...(from && { from: new Date(`${from}T00:00:00`).getTime(), to: until }), limit: pageSize,
  };
  const queryKey = JSON.stringify(query);
  const load = async (number: number, filter: EventQuery = query, refresh = false) => {
    const call = ++serial.current;
    const startedRevision = latestRevision.current;
    const readingHistory = currentPage.current?.page !== 1 || (table.current?.scrollTop ?? 0) > 0 || expanded !== null;
    const anchor = refresh && readingHistory ? visibleRequest(table.current) : null;
    const generation = api.nextEventGeneration();
    retry.current = { page: number, refresh };
    busy.current = true;
    setLoading(true); setLoadingFeedback(!refresh); setError("");
    try {
      const result = await api.readRequestEvents({ ...filter, page: number, ...(anchor && { anchor: anchor.id }) }, generation);
      if (serial.current !== call) return;
      if (!result || !Array.isArray(result.events)) throw new Error("请求事件返回格式无效");
      // 等待期间用户又滚动了表格，保留新阅读位置，下一轮同步再取新的锚点。
      if (anchor && table.current && Math.abs(table.current.scrollTop - anchor.scrollTop) > 1) return;
      restoreView.current = refresh ? anchor : "top";
      currentPage.current = result;
      setPage(result);
      setExpanded(current => refresh && result.events.some(event => event.id === current) ? current : null);
      if (!refresh || document.activeElement !== jumpInput.current) setJump(String(result.page));
    } catch (e) {
      if (serial.current === call) setError(String(e));
    } finally {
      if (serial.current === call) {
        busy.current = false;
        setLoading(false);
        // 后台同步不能打断用户翻页；本次读取结束后再合并一次期间积累的更新。
        if (latestRevision.current !== startedRevision) setQueuedSync(value => value + 1);
      }
    }
  };
  useEffect(() => {
    currentPage.current = null;
    restoreView.current = null;
    setPage(null); setExpanded(null); setNotice(""); setJump("1");
    void load(1, JSON.parse(queryKey) as EventQuery);
    return () => { ++serial.current; busy.current = false; void api.cancelRequestEvents().catch(() => {}); };
    // 筛选或每页条数改变才回到首页；后台同步单独更新当前阅读位置。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryKey]);
  useEffect(() => {
    // 首次查询失败时也允许后续同步恢复，不能只有已成功读取过的页才能刷新。
    if (!busy.current) void load(currentPage.current?.page ?? 1, JSON.parse(queryKey), true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revision, queuedSync]);
  useLayoutEffect(() => {
    const view = restoreView.current, element = table.current;
    restoreView.current = null;
    if (!view || !element) return;
    if (view === "top") { element.scrollTop = 0; return; }
    const row = [...element.querySelectorAll<HTMLTableRowElement>("tr[data-event-id]")].find(row => row.dataset.eventId === view.id && row.classList.contains("request-detail-row") === view.detail);
    if (row) {
      const top = element.getBoundingClientRect().top + (element.querySelector("thead")?.getBoundingClientRect().height ?? 0);
      element.scrollTop += row.getBoundingClientRect().top - top - view.offset;
    }
  }, [page]);
  useEffect(() => {
    onModels((page?.events ?? []).map(({ agent, model }) => ({ agent, model })));
  }, [page, onModels]);
  const names = new Map(agents.map(a => [a.id, a.displayName]));
  const pageCount = page ? Math.max(1, Math.ceil(page.total / page.pageSize)) : 1;
  const goTo = (number: number) => {
    const target = Math.max(1, Math.min(pageCount, Number.isFinite(number) ? Math.trunc(number) : page?.page ?? 1));
    setJump(String(target));
    if (page && target !== page.page && !navigating) { setNotice(""); void load(target); }
  };
  return <section className="usage-section request-events" aria-label="请求事件">
    <div className="request-heading">
      <div className="request-title"><h2>请求事件</h2><HelpTip label="请求事件">
        TPS = 输出 Token（含思考）÷ 请求总耗时。
        <p>“≈”为估算，“—”为未记录；展开行可看详情。</p>
      </HelpTip><span className="request-count">{syncing && !page?.total ? "同步中" : `${page ? formatTokens(page.total) : "—"} 条`}</span></div>
      <div className="request-filters">
        <span className="usage-select"><select aria-label="请求结果" value={status} onChange={e => setStatus(e.target.value)}><option value="all">全部结果</option><option value="success">成功</option><option value="failed">失败</option><option value="unknown">未记录</option></select><UiIcon icon={ChevronDown} /></span>
        {status !== "all" && <button className="request-clear" onClick={() => setStatus("all")}>清除结果筛选</button>}
      </div>
    </div>
    <div className="request-table-wrap" ref={table} tabIndex={0} role="region" aria-label="请求事件表，可横向滚动" aria-busy={loading}>
      {!!page?.events.length && <table className="request-table"><colgroup>
        <col style={{ width: 32 }} /><col style={{ width: 104 }} /><col style={{ width: 140 }} />
        <col style={{ width: 90 }} /><col style={{ width: 126 }} /><col style={{ width: 64 }} />
        <col style={{ width: 116 }} /><col style={{ width: 96 }} /><col style={{ width: 96 }} />
        <col style={{ width: 96 }} /><col style={{ width: 92 }} /><col /><col style={{ width: 176 }} />
      </colgroup><thead><tr>
        <th aria-label="详情" /><th>时间</th><th>模型</th><th>推理 / 速度</th><th>Agent / 来源</th><th>结果</th><th>TPS / 耗时</th><th className="numeric">输入</th><th className="numeric">缓存读取</th><th className="numeric">输出</th><th className="numeric">成本</th><th>项目 / 目录</th><th>会话 ID</th>
      </tr></thead><tbody>
        {(page?.events ?? []).map(event => {
          const r = event.request, date = event.time > 0 ? new Date(event.time) : null;
          const estimated = r.timing === "estimated";
          const client = r.client?.trim() || r.kind?.trim() || "未记录";
          const project = r.project?.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || r.project;
          const input = event.inputRecorded ? `${event.inputIncomplete ? "≥ " : ""}${formatTokens(event.inputTokens)}` : "—";
          const output = event.outputRecorded ? `${event.outputIncomplete ? "≥ " : ""}${formatTokens(event.outputTokens)}` : "—";
          const cachePercent = event.cacheReadRecorded && event.inputRecorded && !event.inputIncomplete && event.inputTokens > 0 && event.cacheReadTokens <= event.inputTokens && event.agent !== "qoder" ? `${(event.cacheReadTokens / event.inputTokens * 100).toFixed(1)}%` : null;
          return <Fragment key={event.id}><tr data-event-id={event.id} className={expanded === event.id ? "request-selected" : undefined}>
            <td><button className="request-expand" aria-label={`查看请求详情 ${event.id}`} aria-expanded={expanded === event.id} onClick={() => setExpanded(expanded === event.id ? null : event.id)}><UiIcon icon={ChevronDown} /></button></td>
            <td>{date ? <><span>{date.toLocaleTimeString("zh-CN", { hour12: false })}</span><small>{dateKey(date).replace(/-/g, "/")}</small></> : "未记录"}</td>
            <td className="request-model"><ModelLabel id={event.model} name={event.model === "unknown" ? "未知模型" : modelLabel(event.agent, event.model, event.modelName)} />{r.provider && <small title={r.provider}>{r.provider}</small>}</td>
            <td><span>{r.effort ?? "—"}</span><small>{r.tier ?? "—"}</small></td>
            <td className="request-source"><span>{names.get(event.agent) ?? event.agent}</span><small title={r.client ? `客户端：${r.client}` : r.kind ? `原始来源类型：${r.kind}` : "未记录客户端"}>{client}</small></td>
            <td><span className={`request-result ${event.status}`}>{resultNames[event.status]}</span>{r.scope === "usage_delta" && <small>累计差额</small>}{r.scope === "error" && <small>错误事件</small>}</td>
            <td className="request-timing" title={r.timing ? timingNames[r.timing] : "日志未记录请求耗时"}><span>{event.outputTps == null ? "—" : `${estimated ? "≈ " : ""}${event.outputTps.toFixed(1)} t/s`}</span><small>{r.latencyMs == null ? "—" : `${estimated ? "≈ " : ""}${duration(r.latencyMs)}`}</small>{r.ttftMs != null && <small title={`${r.ttftScope === "turn" ? "本轮" : "请求"}首 Token：${duration(r.ttftMs)}`}>{r.ttftScope === "turn" ? "本轮首字 " : "首字 "}{duration(r.ttftMs)}</small>}</td>
            <td className="numeric">{input}</td>
            <td className="numeric"><span>{event.cacheReadRecorded ? formatTokens(event.cacheReadTokens) : "—"}</span>{cachePercent && <small>{cachePercent}</small>}</td>
            <td className="numeric"><span>{output}</span>{event.reasoningRecorded && <small>思考 {formatTokens(event.reasoningTokens)}</small>}</td>
            <td className="numeric" title="日志记录的美元成本">{r.costUsd == null ? "—" : `$${r.costUsd.toFixed(6)}`}</td>
            <td className="request-project"><span title={r.project ?? undefined}>{project || "未记录"}</span>{r.project && <small title={r.project}>{shortPath(r.project)}</small>}</td>
            <td><CopyButton className="request-session" value={event.session} title={event.session} label={`复制会话 ID ${event.session}`} onCopied={() => setNotice("")} onError={() => setNotice("复制失败，可以展开详情选择会话 ID")}><code>{event.session}</code></CopyButton></td>
          </tr>{expanded === event.id && <tr className="request-detail-row" data-event-id={event.id}><td colSpan={13}><EventDetail event={event} /></td></tr>}</Fragment>;
        })}
      </tbody></table>}
      {!page?.events.length && !error && <div className="request-empty">{loading ? "正在读取请求事件…" : syncing ? "正在读取历史请求…" : "当前筛选下没有请求事件"}</div>}
    </div>
    {error && <div className="request-error" role="alert">读取失败：{error}<button disabled={navigating} onClick={() => void load(retry.current.page, query, retry.current.refresh)}>重试</button></div>}
    {notice && <div className="request-notice" role="status">{notice}</div>}
    <div className="request-footer">
      <span className="request-range" role="status">{showLoading && <UiIcon icon={LoaderCircle} className="usage-spin" />}{page && (!syncing || page.total > 0) && (page.total ? `第 ${formatTokens((page.page - 1) * page.pageSize + 1)}–${formatTokens((page.page - 1) * page.pageSize + page.events.length)} 条，共 ${formatTokens(page.total)} 条` : "共 0 条")}</span>
      {page && <nav className="request-pagination" aria-label="请求事件分页">
        <span className="usage-select"><select aria-label="每页请求数" value={pageSize} disabled={navigating} onChange={e => setPageSize(Number(e.target.value))}>{[50, 100, 200].map(size => <option key={size} value={size}>{size} 条/页</option>)}</select><UiIcon icon={ChevronDown} /></span>
        <button aria-label="首页" title="首页" disabled={navigating || page.page <= 1} onClick={() => goTo(1)}><UiIcon icon={ChevronsLeft} /></button>
        <button aria-label="上一页" title="上一页" disabled={navigating || page.page <= 1} onClick={() => goTo(page.page - 1)}><UiIcon icon={ChevronLeft} /></button>
        <span className="request-page-number">第 {formatTokens(page.page)} / {formatTokens(pageCount)} 页</span>
        <button aria-label="下一页" title="下一页" disabled={navigating || page.page >= pageCount} onClick={() => goTo(page.page + 1)}><UiIcon icon={ChevronRight} /></button>
        <button aria-label="末页" title="末页" disabled={navigating || page.page >= pageCount} onClick={() => goTo(pageCount)}><UiIcon icon={ChevronsRight} /></button>
        <form className="request-jump" noValidate onSubmit={e => { e.preventDefault(); goTo(jump.trim() ? Number(jump) : page.page); }}>
          <label>前往<input ref={jumpInput} aria-label="请求页码" type="number" inputMode="numeric" min={1} max={pageCount} value={jump} disabled={navigating} onChange={e => setJump(e.target.value)} /></label>
          <button type="submit" disabled={navigating}>跳转</button>
        </form>
      </nav>}
    </div>
  </section>;
}

function EventDetail({ event }: { event: RequestEvent }) {
  const r = event.request;
  const fields = [
    ["会话 ID", event.session], ["请求 ID", r.requestId], ["项目目录", r.project], ["日志文件", r.source],
    ["客户端 / 版本", [r.client, r.version].filter(Boolean).join(" / ")], ["原始来源类型", r.kind], ["结束原因", r.stop],
    ["HTTP 状态", r.httpStatus], ["缓存读取", event.cacheReadRecorded ? formatTokens(event.cacheReadTokens) : null],
    ["缓存写入", event.cacheWriteRecorded ? formatTokens(event.cacheWriteTokens) : null],
    ["思考 Token", event.reasoningRecorded ? formatTokens(event.reasoningTokens) : null],
    ...(event.unclassifiedTokens > 0 ? [["未分项 Token", formatTokens(event.unclassifiedTokens)]] : []),
    ["开始时间", r.startedAt ? new Date(r.startedAt).toLocaleString("zh-CN", { hour12: false }) : null],
    ["结束时间", r.finishedAt ? new Date(r.finishedAt).toLocaleString("zh-CN", { hour12: false }) : null],
    ["耗时口径", r.timing ? timingNames[r.timing] : null],
    ["首 Token", r.ttftMs == null ? null : `${duration(r.ttftMs)}（${r.ttftScope === "turn" ? "整轮" : "请求"}）`],
  ];
  return <div className="request-detail"><dl>{fields.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value == null || value === "" ? "未记录" : value}</dd></div>)}</dl>{event.partial && <p>部分用量未记录。</p>}</div>;
}
