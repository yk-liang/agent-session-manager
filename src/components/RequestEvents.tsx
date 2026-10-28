import { Fragment, useEffect, useRef, useState } from "react";
import { ChevronDown, Copy, LoaderCircle } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import * as api from "../api";
import type { AgentInfo, EventPage, EventQuery, RequestEvent } from "../types";
import { dateKey, firstDate, formatTokens, type UsagePeriod } from "../usageView";
import HelpTip from "./HelpTip";
import UiIcon from "./UiIcon";

interface Props {
  agents: AgentInfo[];
  period: UsagePeriod;
  now: Date;
  agent: string;
  model: string;
  revision: number;
  onAgentChange: (agent: string) => void;
  onModelChange: (model: string) => void;
}
const resultNames = { success: "成功", failed: "失败", unknown: "未记录" };
const duration = (ms: number | null) => ms == null ? "—" : ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(2)} s`;
const timingNames = { recorded: "日志记录耗时", observed: "请求与响应日志的时间差", estimated: "按相邻事件估算，可能包含调度、重试及工具等待" };

export default function RequestEvents({ agents, period, now, agent, model, revision, onAgentChange, onModelChange }: Props) {
  const [status, setStatus] = useState("all");
  const [page, setPage] = useState<EventPage | null>(null);
  const [loading, setLoading] = useState(false);
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
  const load = async (append: boolean, filter: EventQuery = query) => {
    const call = ++serial.current;
    const generation = api.nextEventGeneration();
    setLoading(true); setError("");
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
    if (activeQuery.current !== queryKey) { setPage(null); setExpanded(null); setNotice(""); }
    activeQuery.current = queryKey;
    void load(false, JSON.parse(queryKey) as EventQuery);
    return () => { ++serial.current; void api.cancelRequestEvents().catch(() => {}); };
    // 翻页状态不触发重新加载；筛选、日期或已完成的采集轮次才刷新第一页。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryKey, revision]);
  const copy = async (session: string) => {
    try { await writeText(session); setNotice("已复制完整会话 ID"); }
    catch { setNotice("复制失败，可以展开详情选择会话 ID"); }
  };
  const names = new Map(agents.map(a => [a.id, a.displayName]));
  return <section className="usage-section request-events" aria-label="请求事件">
    <div className="request-heading">
      <div className="request-title"><h2>请求事件</h2><HelpTip label="请求事件">
        本机日志已记录的模型调用。流式分块和继承历史会合并去重；新数据自动补入，删除原会话后仍保留。
        <p>TPS = 输出 Token（含思考）÷ 整个请求耗时，不是纯生成阶段的速度。“≈”表示按事件边界估算；首 Token 若来自 Codex 整轮计时会单独标注。</p>
        <p>“—”表示未记录，不能视为 0；累计差额可能覆盖多次请求，不显示 TPS。成本只展示日志记录的美元金额，不套用估算价格。失败事件也可能是连接或工具错误。</p>
      </HelpTip><span className="request-count">{page ? formatTokens(page.total) : "—"} 条</span></div>
      <div className="request-filters">
        <span className="usage-select"><select aria-label="请求 Agent" value={agent} onChange={e => onAgentChange(e.target.value)}><option value="all">全部 Agent</option>{agents.map(a => <option key={a.id} value={a.id}>{a.displayName}</option>)}</select><UiIcon icon={ChevronDown} /></span>
        <span className="usage-select"><select aria-label="请求模型" value={model} onChange={e => onModelChange(e.target.value)}><option value="all">全部模型</option>{[...new Set([...(page?.models ?? []), ...(model !== "all" ? [model] : [])])].map(m => <option key={m} value={m}>{m === "unknown" ? "未知模型" : m}</option>)}</select><UiIcon icon={ChevronDown} /></span>
        <span className="usage-select"><select aria-label="请求结果" value={status} onChange={e => setStatus(e.target.value)}><option value="all">全部结果</option><option value="success">成功</option><option value="failed">失败</option><option value="unknown">未记录</option></select><UiIcon icon={ChevronDown} /></span>
        {(agent !== "all" || model !== "all" || status !== "all") && <button className="request-clear" onClick={() => { onAgentChange("all"); onModelChange("all"); setStatus("all"); }}>清除筛选</button>}
      </div>
    </div>
    <div className="request-table-wrap" tabIndex={0} role="region" aria-label="请求事件表，可横向滚动" aria-busy={loading}>
      <table className="request-table"><thead><tr>
        <th aria-label="详情" /><th>时间</th><th>模型</th><th>推理 / 速度</th><th>Agent / 来源</th><th>结果</th><th className="numeric">TPS / 耗时</th><th className="numeric">输入</th><th className="numeric">缓存读取</th><th className="numeric">输出</th><th className="numeric">成本</th><th>会话 ID</th>
      </tr></thead><tbody>
        {(page?.events ?? []).map(event => {
          const r = event.request, date = event.time > 0 ? new Date(event.time) : null;
          const estimated = r.timing === "estimated";
          const input = event.inputRecorded ? `${event.inputIncomplete ? "≥ " : ""}${formatTokens(event.inputTokens)}` : "—";
          const output = event.outputRecorded ? `${event.outputIncomplete ? "≥ " : ""}${formatTokens(event.outputTokens)}` : "—";
          const cachePercent = event.cacheReadRecorded && event.inputRecorded && !event.inputIncomplete && event.inputTokens > 0 && event.cacheReadTokens <= event.inputTokens && event.agent !== "qoder" ? `${(event.cacheReadTokens / event.inputTokens * 100).toFixed(1)}%` : null;
          return <Fragment key={event.id}><tr className={expanded === event.id ? "request-selected" : undefined}>
            <td><button className="request-expand" aria-label={`查看请求详情 ${event.id}`} aria-expanded={expanded === event.id} onClick={() => setExpanded(expanded === event.id ? null : event.id)}><UiIcon icon={ChevronDown} /></button></td>
            <td>{date ? <><span>{date.toLocaleTimeString("zh-CN", { hour12: false })}</span><small>{dateKey(date).replace(/-/g, "/")}</small></> : "未记录"}</td>
            <td className="request-model"><span title={event.model}>{event.model === "unknown" ? "未知模型" : event.modelName}</span>{r.provider && <small title={r.provider}>{r.provider}</small>}</td>
            <td><span>{r.effort ?? "—"}</span><small>{r.tier ?? "—"}</small></td>
            <td className="request-source"><span>{names.get(event.agent) ?? event.agent}</span><small title={r.kind ?? r.client ?? ""}>{r.kind ?? r.client ?? "—"}</small></td>
            <td><span className={`request-result ${event.status}`}>{resultNames[event.status]}</span>{r.scope === "usage_delta" && <small>累计差额</small>}{r.scope === "error" && <small>错误事件</small>}</td>
            <td className="numeric" title={r.timing ? timingNames[r.timing] : "日志未记录请求耗时"}><span>{event.outputTps == null ? "—" : `${estimated ? "≈ " : ""}${event.outputTps.toFixed(1)} t/s`}</span><small>{r.latencyMs == null ? "—" : `${estimated ? "≈ " : ""}${duration(r.latencyMs)}`}</small>{r.ttftMs != null && <small title="从本轮开始到首个 Token，可能包含请求前准备">{r.ttftScope === "turn" ? "本轮首 Token " : "首 Token "}{duration(r.ttftMs)}</small>}</td>
            <td className="numeric">{input}</td>
            <td className="numeric"><span>{event.cacheReadRecorded ? formatTokens(event.cacheReadTokens) : "—"}</span>{cachePercent && <small>{cachePercent}</small>}</td>
            <td className="numeric"><span>{output}</span>{event.reasoningRecorded && <small>思考 {formatTokens(event.reasoningTokens)}</small>}</td>
            <td className="numeric" title="日志记录的美元成本">{r.costUsd == null ? "—" : `$${r.costUsd.toFixed(6)}`}</td>
            <td><button className="request-session" onClick={() => void copy(event.session)} title={event.session} aria-label={`复制会话 ID ${event.session}`}><code>{event.session}</code><UiIcon icon={Copy} size={12} /></button></td>
          </tr>{expanded === event.id && <tr className="request-detail-row"><td colSpan={12}><EventDetail event={event} /></td></tr>}</Fragment>;
        })}
      </tbody></table>
      {!page?.events.length && !error && <div className="request-empty">{loading ? "正在读取请求事件…" : "当前筛选下没有请求事件"}</div>}
    </div>
    {error && <div className="request-error" role="alert">读取失败：{error}<button onClick={() => void load(false)}>重试</button></div>}
    <div className="request-footer"><span role="status">{notice}</span><span>{loading && <UiIcon icon={LoaderCircle} className="usage-spin" />}{page && `已加载 ${formatTokens(page.events.length)} / ${formatTokens(page.total)} 条`}</span>{page?.nextCursor && <button disabled={loading} onClick={() => void load(true)}>加载更多</button>}</div>
  </section>;
}

function EventDetail({ event }: { event: RequestEvent }) {
  const r = event.request;
  const fields = [
    ["会话 ID", event.session], ["请求 ID", r.requestId], ["项目", r.project], ["日志文件", r.source],
    ["客户端 / 版本", [r.client, r.version].filter(Boolean).join(" / ")], ["结束原因", r.stop],
    ["HTTP 状态", r.httpStatus], ["缓存写入", event.cacheWriteRecorded ? formatTokens(event.cacheWriteTokens) : null],
    ["未分项 Token", event.unclassifiedTokens > 0 ? formatTokens(event.unclassifiedTokens) : null],
    ["开始时间", r.startedAt ? new Date(r.startedAt).toLocaleString("zh-CN", { hour12: false }) : null],
    ["结束时间", r.finishedAt ? new Date(r.finishedAt).toLocaleString("zh-CN", { hour12: false }) : null],
    ["耗时口径", r.timing ? timingNames[r.timing] : null],
  ];
  return <div className="request-detail"><dl>{fields.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value == null || value === "" ? "未记录" : value}</dd></div>)}</dl>{event.partial && <p>此记录的计量可能不完整，缺失字段没有按 0 补齐。</p>}</div>;
}
