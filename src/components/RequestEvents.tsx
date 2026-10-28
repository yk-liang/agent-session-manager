import { Fragment, useEffect, useRef, useState } from "react";
import { ChevronDown, LoaderCircle } from "lucide-react";
import CopyButton from "./CopyButton";
import { useDelayedBusy } from "../useDelayedBusy";
import * as api from "../api";
import type { AgentInfo, EventPage, EventQuery, RequestEvent } from "../types";
import { dateKey, firstDate, formatTokens, type UsagePeriod } from "../usageView";
import { shortPath } from "../utils";
import HelpTip from "./HelpTip";
import UiIcon from "./UiIcon";

interface Props {
  agents: AgentInfo[];
  period: UsagePeriod;
  now: Date;
  agent: string;
  model: string;
  revision: number;
  syncing?: boolean;
}
const resultNames = { success: "成功", failed: "失败", unknown: "未记录" };
const duration = (ms: number | null) => ms == null ? "—" : ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(2)} s`;
const timingNames = { recorded: "日志记录耗时", observed: "请求与响应日志的时间差", estimated: "按相邻事件估算，可能包含调度、重试及工具等待" };

export default function RequestEvents({ agents, period, now, agent, model, revision, syncing = false }: Props) {
  const [status, setStatus] = useState("all");
  const [page, setPage] = useState<EventPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadingFeedback, setLoadingFeedback] = useState(false);
  const showLoading = useDelayedBusy(loading && loadingFeedback);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);
  const serial = useRef(0);
  const from = firstDate(period, now);
  const until = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1).getTime();
  const query: EventQuery = {
    ...(agent !== "all" && { agent }), ...(model !== "all" && { model }), ...(status !== "all" && { status }),
    ...(from && { from: new Date(`${from}T00:00:00`).getTime(), to: until }), limit: 50,
  };
  const queryKey = JSON.stringify(query);
  const activeQuery = useRef("");
  const load = async (append: boolean, filter: EventQuery = query, feedback = true) => {
    const call = ++serial.current;
    const generation = api.nextEventGeneration();
    setLoading(true); setLoadingFeedback(feedback); setError("");
    try {
      const result = await api.readRequestEvents({ ...filter, ...(append && page?.nextCursor && { cursor: page.nextCursor }) }, generation);
      if (serial.current !== call) return;
      if (!result || !Array.isArray(result.events)) throw new Error("请求事件返回格式无效");
      setPage(previous => append && !result.reset && previous ? { ...result, events: [...previous.events, ...result.events.filter(e => !previous.events.some(p => p.id === e.id))] } : result);
      if (result.reset) setNotice("检测到新事件，已更新到最新一页");
    } catch (e) {
      if (serial.current === call) setError(String(e));
    } finally {
      if (serial.current === call) setLoading(false);
    }
  };
  useEffect(() => {
    const changed = activeQuery.current !== queryKey;
    if (changed) { setPage(null); setExpanded(null); setNotice(""); }
    activeQuery.current = queryKey;
    // 自动补入请求时保留当前表格；用户切换筛选或首次读取才需要加载提示。
    void load(false, JSON.parse(queryKey) as EventQuery, changed || page === null);
    return () => { ++serial.current; void api.cancelRequestEvents().catch(() => {}); };
    // 翻页状态不触发重新加载；筛选、日期或已完成的采集轮次才刷新第一页。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryKey, revision]);
  const names = new Map(agents.map(a => [a.id, a.displayName]));
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
    <div className="request-table-wrap" tabIndex={0} role="region" aria-label="请求事件表，可横向滚动" aria-busy={loading}>
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
          return <Fragment key={event.id}><tr className={expanded === event.id ? "request-selected" : undefined}>
            <td><button className="request-expand" aria-label={`查看请求详情 ${event.id}`} aria-expanded={expanded === event.id} onClick={() => setExpanded(expanded === event.id ? null : event.id)}><UiIcon icon={ChevronDown} /></button></td>
            <td>{date ? <><span>{date.toLocaleTimeString("zh-CN", { hour12: false })}</span><small>{dateKey(date).replace(/-/g, "/")}</small></> : "未记录"}</td>
            <td className="request-model"><span title={event.model}>{event.model === "unknown" ? "未知模型" : event.modelName}</span>{r.provider && <small title={r.provider}>{r.provider}</small>}</td>
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
          </tr>{expanded === event.id && <tr className="request-detail-row"><td colSpan={13}><EventDetail event={event} /></td></tr>}</Fragment>;
        })}
      </tbody></table>}
      {!page?.events.length && !error && <div className="request-empty">{loading ? "正在读取请求事件…" : syncing ? "正在读取历史请求…" : "当前筛选下没有请求事件"}</div>}
    </div>
    {error && <div className="request-error" role="alert">读取失败：{error}<button onClick={() => void load(false)}>重试</button></div>}
    <div className="request-footer"><span role="status">{notice}</span><span>{showLoading && <UiIcon icon={LoaderCircle} className="usage-spin" />}{page && (!syncing || page.total > 0) && `已加载 ${formatTokens(page.events.length)} / ${formatTokens(page.total)} 条`}</span>{page?.nextCursor && <button disabled={loading} onClick={() => void load(true)}>加载更多</button>}</div>
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
