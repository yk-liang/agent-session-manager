import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ChevronDown, ChevronFirst, ChevronLast, ChevronLeft, ChevronRight, LoaderCircle } from "lucide-react";
import CopyButton from "./CopyButton";
import { useDelayedBusy } from "../useDelayedBusy";
import * as api from "../api";
import type { AgentInfo, EventPage, EventQuery, RequestEvent } from "../types";
import { dateKey, firstDate, formatTokens, type UsagePeriod } from "../usageView";
import { shortPath } from "../utils";
import HelpTip from "./HelpTip";
import UiIcon from "./UiIcon";
import ModelLabel from "./ModelLabel";
import AgentIcon from "./AgentIcon";
import { ProviderIcon } from "./BrandIcon";
import Dropdown from "./Dropdown";

interface Props {
  agents: AgentInfo[];
  period: UsagePeriod;
  now: Date;
  agent: string;
  model: string;
  revision: number;
  syncing?: boolean;
}
const statusLabels = { success: "成功", failed: "失败", unknown: "未记录" };
const statusOptions = [{ id: "all", name: "全部结果" }, ...Object.entries(statusLabels).map(([id, name]) => ({ id, name }))];
const pageSizeOptions = [50, 100, 200].map(size => ({ id: String(size), name: `${size} 条/页` }));
const formatDuration = (ms: number | null) => ms == null ? "—" : ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(2)} s`;
const timingLabels = { recorded: "日志记录的耗时", observed: "请求与响应日志的时间差", estimated: "按相邻日志估算，可能包含调度、重试和工具等待时间" };
interface ViewAnchor { id: string; detail: boolean; offset: number; scrollTop: number }

function visibleRequest(table: HTMLDivElement | null): ViewAnchor | null {
  if (!table) return null;
  const top = table.getBoundingClientRect().top + (table.querySelector("thead")?.getBoundingClientRect().height ?? 0);
  const row = [...table.querySelectorAll<HTMLTableRowElement>("tr[data-event-id]")].find(row => row.getBoundingClientRect().bottom > top);
  return row ? { id: row.dataset.eventId!, detail: row.classList.contains("request-detail-row"), offset: row.getBoundingClientRect().top - top, scrollTop: table.scrollTop } : null;
}

export default function RequestEvents({ agents, period, now, agent, model, revision, syncing = false }: Props) {
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
  const [expandedRequestId, setExpandedRequestId] = useState<string | null>(null);
  const requestSequence = useRef(0);
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
  const loadPage = async (pageNumber: number, pageQuery: EventQuery = query, refresh = false) => {
    const requestId = ++requestSequence.current;
    const startedRevision = latestRevision.current;
    const readingHistory = currentPage.current?.page !== 1 || (table.current?.scrollTop ?? 0) > 0 || expandedRequestId !== null;
    const anchor = refresh && readingHistory ? visibleRequest(table.current) : null;
    const generation = api.nextEventGeneration();
    retry.current = { page: pageNumber, refresh };
    busy.current = true;
    setLoading(true); setLoadingFeedback(!refresh); setError("");
    try {
      const result = await api.readRequestEvents({ ...pageQuery, page: pageNumber, ...(anchor && { anchor: anchor.id }) }, generation);
      if (requestSequence.current !== requestId) return;
      if (!result || !Array.isArray(result.events)) throw new Error("请求记录格式无效");
      // 用户已滚动到新位置时，跳过这次刷新。
      if (anchor && table.current && Math.abs(table.current.scrollTop - anchor.scrollTop) > 1) return;
      restoreView.current = refresh ? anchor : "top";
      currentPage.current = result;
      setPage(result);
      setExpandedRequestId(current => refresh && result.events.some(event => event.id === current) ? current : null);
      if (!refresh || document.activeElement !== jumpInput.current) setJump(String(result.page));
    } catch (e) {
      if (requestSequence.current === requestId) setError(String(e));
    } finally {
      if (requestSequence.current === requestId) {
        busy.current = false;
        setLoading(false);
        // 翻页结束后补查这期间同步的新记录。
        if (latestRevision.current !== startedRevision) setQueuedSync(value => value + 1);
      }
    }
  };
  useEffect(() => {
    currentPage.current = null;
    restoreView.current = null;
    setPage(null); setExpandedRequestId(null); setNotice(""); setJump("1");
    void loadPage(1, JSON.parse(queryKey) as EventQuery);
    return () => { ++requestSequence.current; busy.current = false; void api.cancelRequestEvents().catch(() => {}); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryKey]);
  useEffect(() => {
    if (!busy.current) void loadPage(currentPage.current?.page ?? 1, JSON.parse(queryKey), true);
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
  const agentNames = new Map(agents.map(a => [a.id, a.displayName]));
  const pageCount = page ? Math.max(1, Math.ceil(page.total / page.pageSize)) : 1;
  const goTo = (number: number) => {
    const target = Math.max(1, Math.min(pageCount, Number.isFinite(number) ? Math.trunc(number) : page?.page ?? 1));
    setJump(String(target));
    if (page && target !== page.page && !navigating) { setNotice(""); void loadPage(target); }
  };
  return <section className="usage-section request-events" aria-label="请求记录">
    <div className="request-heading">
      <div className="request-title"><h2>请求记录</h2><HelpTip label="请求记录">
        TPS = 输出 Token（含思考）÷ 请求总耗时。
        <p>“≈”表示估算值，“—”表示未记录。</p>
      </HelpTip><span className="request-count">{syncing && !page?.total ? "同步中" : `${page ? formatTokens(page.total) : "—"} 条`}</span></div>
      <div className="request-filters">
        <Dropdown label="请求结果" value={status} options={statusOptions} onChange={setStatus} />
        {status !== "all" && <button className="request-clear" onClick={() => setStatus("all")}>清除筛选</button>}
      </div>
    </div>
    <div className="request-table-wrap" ref={table} tabIndex={0} role="region" aria-label="请求记录表" aria-busy={loading}>
      {!!page?.events.length && <table className="request-table"><colgroup>
        <col style={{ width: 32 }} /><col style={{ width: 104 }} /><col style={{ width: 140 }} />
        <col style={{ width: 90 }} /><col style={{ width: 126 }} /><col style={{ width: 64 }} />
        <col style={{ width: 116 }} /><col style={{ width: 96 }} /><col style={{ width: 96 }} />
        <col style={{ width: 96 }} /><col style={{ width: 92 }} /><col /><col style={{ width: 176 }} />
      </colgroup><thead><tr>
        <th aria-label="详情" /><th>时间</th><th>模型</th><th>推理 / 档位</th><th>Agent / 来源</th><th>结果</th><th>TPS / 耗时</th><th className="numeric">输入</th><th className="numeric">缓存读取</th><th className="numeric">输出</th><th className="numeric">成本</th><th>项目 / 目录</th><th>会话 ID</th>
      </tr></thead><tbody>
        {(page?.events ?? []).map(event => {
          const request = event.request, date = event.time > 0 ? new Date(event.time) : null;
          const estimated = request.timing === "estimated";
          const client = request.client?.trim() || request.kind?.trim() || "未记录";
          const project = request.project?.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || request.project;
          const input = event.inputRecorded ? `${event.inputIncomplete ? "≥ " : ""}${formatTokens(event.inputTokens)}` : "—";
          const output = event.outputRecorded ? `${event.outputIncomplete ? "≥ " : ""}${formatTokens(event.outputTokens)}` : "—";
          const cachePercent = event.cacheReadRecorded && event.inputRecorded && !event.inputIncomplete && event.inputTokens > 0 && event.cacheReadTokens <= event.inputTokens && event.agent !== "qoder" ? `${(event.cacheReadTokens / event.inputTokens * 100).toFixed(1)}%` : null;
          return <Fragment key={event.id}><tr data-event-id={event.id} className={expandedRequestId === event.id ? "request-selected" : undefined}>
            <td><button className="request-expand" aria-label={`查看请求详情 ${event.id}`} title={expandedRequestId === event.id ? "收起详情" : "查看详情"} aria-expanded={expandedRequestId === event.id} onClick={() => setExpandedRequestId(expandedRequestId === event.id ? null : event.id)}>
              <UiIcon icon={expandedRequestId === event.id ? ChevronDown : ChevronRight} />
            </button></td>
            <td>{date ? <><span>{date.toLocaleTimeString("zh-CN", { hour12: false })}</span><small>{dateKey(date).replace(/-/g, "/")}</small></> : "未记录"}</td>
            <td className="request-model"><ModelLabel model={event.model} />{request.provider && <small title={request.provider}><ProviderIcon provider={request.provider} />{request.provider}</small>}</td>
            <td><span>{request.effort ?? "—"}</span><small>{request.tier ?? "—"}</small></td>
            <td className="request-source"><span><AgentIcon agent={event.agent} inline />{agentNames.get(event.agent) ?? event.agent}</span><small title={request.client ? `客户端：${request.client}` : request.kind ? `日志来源类型：${request.kind}` : "未记录客户端"}>{client}</small></td>
            <td><span className={`request-result ${event.status}`}>{statusLabels[event.status]}</span>{request.scope === "usage_delta" && <small>累计差额</small>}{request.scope === "error" && <small>错误事件</small>}</td>
            <td className="request-timing" title={request.timing ? timingLabels[request.timing] : "日志未记录请求耗时"}><span>{event.outputTps == null ? "—" : `${estimated ? "≈ " : ""}${event.outputTps.toFixed(1)} t/s`}</span><small>{request.latencyMs == null ? "—" : `${estimated ? "≈ " : ""}${formatDuration(request.latencyMs)}`}</small>{request.ttftMs != null && <small title={`${request.ttftScope === "turn" ? "本轮" : "请求"}首 Token：${formatDuration(request.ttftMs)}`}>{request.ttftScope === "turn" ? "本轮首 Token " : "首 Token "}{formatDuration(request.ttftMs)}</small>}</td>
            <td className="numeric">{input}</td>
            <td className="numeric"><span>{event.cacheReadRecorded ? formatTokens(event.cacheReadTokens) : "—"}</span>{cachePercent && <small>{cachePercent}</small>}</td>
            <td className="numeric"><span>{output}</span>{event.reasoningRecorded && <small>思考 {formatTokens(event.reasoningTokens)}</small>}</td>
            <td className="numeric" title="日志记录的美元成本">{request.costUsd == null ? "—" : `$${request.costUsd.toFixed(6)}`}</td>
            <td className="request-project"><span title={request.project ?? undefined}>{project || "未记录"}</span>{request.project && <small title={request.project}>{shortPath(request.project)}</small>}</td>
            <td><CopyButton className="request-session" value={event.session} title={event.session} label={`复制会话 ID ${event.session}`} onCopied={() => setNotice("")} onError={() => setNotice("复制失败。可在详情中选择会话 ID。")}><code>{event.session}</code></CopyButton></td>
          </tr>{expandedRequestId === event.id && <tr className="request-detail-row" data-event-id={event.id}><td colSpan={13}><EventDetail event={event} /></td></tr>}</Fragment>;
        })}
      </tbody></table>}
      {!page?.events.length && !error && <div className="request-empty">{loading ? "正在读取请求记录…" : syncing ? "正在读取历史请求…" : "没有匹配的请求"}</div>}
    </div>
    {error && <div className="request-error" role="alert">读取失败：{error}<button disabled={navigating} onClick={() => void loadPage(retry.current.page, query, retry.current.refresh)}>重试</button></div>}
    {notice && <div className="request-notice" role="status">{notice}</div>}
    <div className="request-footer">
      <span className="request-range" role="status">{showLoading && <UiIcon icon={LoaderCircle} className="usage-spin" />}{page && (!syncing || page.total > 0) && (page.total ? `第 ${formatTokens((page.page - 1) * page.pageSize + 1)}–${formatTokens((page.page - 1) * page.pageSize + page.events.length)} 条，共 ${formatTokens(page.total)} 条` : "共 0 条")}</span>
      {page && <nav className="request-pagination" aria-label="请求记录分页">
        <Dropdown label="每页请求数" value={String(pageSize)} options={pageSizeOptions}
          disabled={navigating} onChange={id => setPageSize(Number(id))} />
        <div className="request-page-controls">
          <button aria-label="首页" title="首页" disabled={navigating || page.page <= 1} onClick={() => goTo(1)}><UiIcon icon={ChevronFirst} /></button>
          <button aria-label="上一页" title="上一页" disabled={navigating || page.page <= 1} onClick={() => goTo(page.page - 1)}><UiIcon icon={ChevronLeft} /></button>
          <span className="request-page-number">第 {formatTokens(page.page)} / {formatTokens(pageCount)} 页</span>
          <button aria-label="下一页" title="下一页" disabled={navigating || page.page >= pageCount} onClick={() => goTo(page.page + 1)}><UiIcon icon={ChevronRight} /></button>
          <button aria-label="末页" title="末页" disabled={navigating || page.page >= pageCount} onClick={() => goTo(pageCount)}><UiIcon icon={ChevronLast} /></button>
        </div>
        <form className="request-jump" noValidate onSubmit={e => { e.preventDefault(); goTo(jump.trim() ? Number(jump) : page.page); }}>
          <label>前往<input ref={jumpInput} className="control-input" aria-label="请求页码" type="number" inputMode="numeric" min={1} max={pageCount} value={jump} disabled={navigating} onChange={e => setJump(e.target.value)} /></label>
          <button type="submit" disabled={navigating}>跳转</button>
        </form>
      </nav>}
    </div>
  </section>;
}

function EventDetail({ event }: { event: RequestEvent }) {
  const request = event.request;
  const fields = [
    ["会话 ID", event.session], ["请求 ID", request.requestId], ["项目目录", request.project], ["日志文件", request.source],
    ["客户端 / 版本", [request.client, request.version].filter(Boolean).join(" / ")], ["日志来源类型", request.kind], ["结束原因", request.stop],
    ["HTTP 状态", request.httpStatus], ["缓存读取", event.cacheReadRecorded ? formatTokens(event.cacheReadTokens) : null],
    ["缓存写入", event.cacheWriteRecorded ? formatTokens(event.cacheWriteTokens) : null],
    ["思考 Token", event.reasoningRecorded ? formatTokens(event.reasoningTokens) : null],
    ...(event.unclassifiedTokens > 0 ? [["未分项 Token", formatTokens(event.unclassifiedTokens)]] : []),
    ["开始时间", request.startedAt ? new Date(request.startedAt).toLocaleString("zh-CN", { hour12: false }) : null],
    ["结束时间", request.finishedAt ? new Date(request.finishedAt).toLocaleString("zh-CN", { hour12: false }) : null],
    ["耗时来源", request.timing ? timingLabels[request.timing] : null],
    ["首 Token 耗时", request.ttftMs == null ? null : `${formatDuration(request.ttftMs)}（${request.ttftScope === "turn" ? "整轮" : "请求"}）`],
  ];
  return <div className="request-detail"><dl>{fields.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value == null || value === "" ? "未记录" : value}</dd></div>)}</dl>{event.partial && <p>部分用量未记录。</p>}</div>;
}
