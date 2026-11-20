import { Fragment, memo } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { AgentInfo } from "../types";
import type { EventPage, RequestEvent } from "./types";
import { formatClock, formatCount, formatDate, formatDuration, formatPercent, formatRate, formatTps, formatUsd } from "./format";
import { costBreakdown, eventCostDisplay, requestErrorText, tokenDisplay, uncachedInputOf } from "./metrics";
import { REQUEST_OUTCOMES } from "./trend";
import { projectName, shortPath } from "../utils";
import AgentIcon from "../components/AgentIcon";
import { ProviderIcon } from "../components/BrandIcon";
import { TipTrigger, type TooltipContent } from "./UsageTooltip";
import CopyButton from "../components/CopyButton";
import ModelLabel from "../components/ModelLabel";
import RequestDetail from "./RequestDetail";
import UiIcon from "../components/UiIcon";
import UsageNumber from "./UsageNumber";

function requestColumns(cacheWrites: boolean, serviceTiers: boolean): { label: string; srOnly?: boolean; numeric?: boolean }[] {
  return [
    { label: "详情", srOnly: true },
    { label: "时间" },
    { label: "模型" },
    { label: serviceTiers ? "推理 / 档位" : "推理" },
    { label: "Agent / 客户端" },
    { label: "结果" },
    { label: "TPS / 耗时" },
    { label: "输入", numeric: true },
    { label: "缓存读取", numeric: true },
    ...(cacheWrites ? [{ label: "缓存写入", numeric: true }] : []),
    { label: "输出", numeric: true },
    { label: "费用", numeric: true },
    { label: "项目" },
    { label: "会话" },
  ];
}
function costTip(event: RequestEvent): TooltipContent {
  const shown = eventCostDisplay(event);
  if (shown.missing) return { title: "费用无法估算", rows: [], note: shown.title };
  const total = { label: "合计", value: shown.text };
  if (event.cost?.source === "builtin") return { title: shown.title, rows: [], total };
  return {
    title: "估算明细",
    rows: costBreakdown(event).map(([label, tokens, rate, usd]) => ({
      key: label, label: `${label} ${formatCount(tokens)} × ${formatRate(rate)}`, value: usd == null ? "—" : formatUsd(usd),
    })),
    total,
    note: shown.title,
  };
}

export default memo(function RequestTable({ page, agents, expandedRequestId, onExpand, copyFeedback }: {
  page: EventPage;
  agents: AgentInfo[];
  expandedRequestId: string | null;
  onExpand: (id: string | null) => void;
  copyFeedback: { onCopied: () => void; onError: () => void };
}) {
  const agentNames = new Map(agents.map(agent => [agent.id, agent.displayName]));
  const showCacheWrites = page.hasCacheWrites;
  const columns = requestColumns(showCacheWrites, page.hasServiceTiers);
  const { events } = page;

  return (
    <table className="request-table">
        <thead><tr>{columns.map(({ label, srOnly, numeric }) => srOnly
          ? <th key={label} aria-label={label} />
          : <th key={label} scope="col" className={numeric ? "numeric" : undefined}>{label}</th>)}</tr></thead>
        <tbody>
          {events.map(event => {
            const { request } = event;
            const { latencyMs, ttftMs } = request;
            const agentName = agentNames.get(event.agent) ?? event.agent;
            const tier = request.tier?.trim();
            const estimated = request.timing === "codex_boundary" && latencyMs != null;
            const expanded = expandedRequestId === event.id;
            const session = request.subagentId || event.session;
            const project = request.absoluteProjectPath ?? request.project;
            const client = request.client?.trim();
            const cachePercent = event.cacheReadRecorded && event.inputRecorded && !event.inputIncomplete && event.inputTokens > 0 &&
              event.cacheReadTokens <= event.inputTokens && event.agent !== "qoder" ? (event.cacheReadTokens / event.inputTokens) * 100 : null;
            const uncached = event.agent === "qoder" ? null : uncachedInputOf(event);
            const write1h = Math.min(event.cacheWrite1hTokens ?? 0, event.cacheWriteTokens);
            const cost = eventCostDisplay(event);
            const result = <span className="status-tag request-result" data-tone={event.status === "success" ? "success" : "error"}>{REQUEST_OUTCOMES[event.status].label}</span>;
            return <Fragment key={event.id}><tr data-event-id={event.id} className={expanded ? "request-selected" : undefined}>
              <td><button type="button" className="request-expand btn-icon btn-ghost" aria-label={`${expanded ? "收起" : "查看"}请求详情 ${event.id}`} title={expanded ? "收起详情" : "查看详情"} aria-expanded={expanded} onClick={() => onExpand(expanded ? null : event.id)}>
                <UiIcon icon={expanded ? ChevronDown : ChevronRight} />
              </button></td>
              <td>{event.time > 0 ? <><span>{formatClock(event.time)}</span><small>{formatDate(event.time)}</small></> : <span className="request-missing">未记录</span>}</td>
              <td className="request-model"><ModelLabel model={event.model} />{request.provider && <small title={request.provider}><ProviderIcon provider={request.provider} />{request.provider}</small>}</td>
              <td><span>{request.effort ?? "—"}</span>{tier && <small className="request-tier">{tier}</small>}</td>
              <td className="request-source"><span><AgentIcon agent={event.agent} inline />{agentName}</span><small title={client}>{client || "未记录"}</small></td>
              <td>
                {event.status === "failed" ? (
                  <TipTrigger content={() => ({ title: "错误信息", rows: [], detail: requestErrorText(request) || "未记录错误信息" })}>
                    {result}
                  </TipTrigger>
                ) : result}
                {request.scope === "usage_delta" && <small title="仅计入 Token 用量">累计用量差额</small>}
              </td>
              <td className="request-timing" title={estimated ? "根据 Codex 日志时间估算" : undefined}>
                <span>{event.outputTps == null ? "—" : `${estimated ? "~" : ""}${formatTps(event.outputTps)}`}</span>
                <small>
                  {ttftMs != null && <span title={request.ttftScope === "turn" ? "整轮对话的首字延迟" : "首字延迟"}>
                    {formatDuration(ttftMs, true)}
                  </span>}
                  {ttftMs != null && latencyMs != null && " · "}
                  {latencyMs == null ? ttftMs == null && "—" : `${estimated ? "~" : ""}${formatDuration(latencyMs, true)}`}
                </small>
              </td>
              <td className="numeric"><UsageNumber value={tokenDisplay(event, "input", { compact: false })} />{uncached != null && event.cacheReadTokens + event.cacheWriteTokens > 0 && <small title="输入减去缓存读写">未缓存 {formatCount(uncached)}</small>}</td>
              <td className="numeric"><UsageNumber value={tokenDisplay(event, "cacheRead", { compact: false })} />{cachePercent != null && <small title="缓存读取 ÷ 输入">{formatPercent(cachePercent)}</small>}</td>
              {showCacheWrites && <td className="numeric"><UsageNumber value={tokenDisplay(event, "cacheWrite", { compact: false })} />{write1h > 0 && <small title="1 小时缓存写入">1h {formatCount(write1h)}</small>}</td>}
              <td className="numeric"><UsageNumber value={tokenDisplay(event, "output", { compact: false })} />{event.reasoningRecorded && event.reasoningTokens > 0 && <small>推理 {formatCount(event.reasoningTokens)}</small>}</td>
              <td className="numeric request-cost">
                <TipTrigger content={() => costTip(event)} label={`费用 ${cost.text}，${cost.title}`}><UsageNumber value={cost} showTitle={false} /></TipTrigger>
                <small>{cost.note}</small>
              </td>
              <td className="request-project">
                <span title={project ?? undefined}>{project ? projectName(project) : "未记录"}</span>
                {project && <small title={project}>{shortPath(project)}</small>}
              </td>
              <td>
                <div className="request-session-id">
                  <code title={session}>{session}</code>
                  <CopyButton className="request-session btn-icon btn-ghost" value={session} label={`复制会话 ID ${session}`} {...copyFeedback} />
                </div>
              </td>
            </tr>
              {expanded && (
                <tr className="request-detail-row" data-event-id={event.id}>
                  <td colSpan={columns.length}>
                    <RequestDetail event={event} agentName={agentName} session={session} project={project} copyFeedback={copyFeedback} />
                  </td>
                </tr>
              )}
            </Fragment>;
          })}
        </tbody>
      </table>
  );
});
