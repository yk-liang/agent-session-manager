import type { RequestEvent, RequestMetadata } from "./types";
import { formatDateTime } from "./format";
import { requestErrorText } from "./metrics";
import { shortPath } from "../utils";
import CopyButton from "../components/CopyButton";

const routineStops = new Set(["end_turn", "stop", "tool_use", "tool-calls"]);
const stopLabels = new Map([
  ["max_tokens", "达到输出上限"],
  ["length", "达到输出上限"],
  ["stop_sequence", "命中停止序列"],
  ["refusal", "模型拒绝"],
  ["content-filter", "触发内容过滤"],
  ["content_filter", "触发内容过滤"],
  ["aborted", "请求取消"],
]);

function requestTime({ startedAt, finishedAt }: RequestMetadata) {
  const format = (time: number) => `${formatDateTime(time)}.${String(time % 1000).padStart(3, "0")}`;
  if (startedAt && finishedAt) return `${format(startedAt)} – ${format(finishedAt)}`;
  if (startedAt) return `开始 ${format(startedAt)}`;
  if (finishedAt) return `结束 ${format(finishedAt)}`;
}

export default function RequestDetail({ event, agentName, copyFeedback }: {
  event: RequestEvent;
  agentName: string;
  copyFeedback: { onCopied: () => void; onError: () => void };
}) {
  const { request } = event;
  const failed = event.status === "failed";
  const error = requestErrorText(request);
  const timing = [
    request.timing === "codex_boundary" && request.latencyMs != null && "耗时根据日志估算",
    request.ttftScope === "turn" && request.ttftMs != null && "首字延迟按整轮对话计算",
  ].filter(Boolean).join("，");
  const stop = request.stop && !routineStops.has(request.stop) && (stopLabels.get(request.stop) ?? request.stop);
  const version = request.version?.trim();
  const client = [request.client?.trim() || (version ? agentName : ""), version].filter(Boolean).join(" ");
  const copyValue = (value: string, label: string, display = value) => (
    <div className="request-detail-copy">
      <code>{display}</code>
      <CopyButton className="request-copy" value={value} label={`复制${label}`} {...copyFeedback} />
    </div>
  );
  const fields = ([
    ["错误信息", failed && (error ? (
      <div className="request-detail-copy request-failure-detail" role="region" aria-label="错误信息">
        <pre>{error}</pre>
        <CopyButton className="request-copy" value={error} label="复制错误信息" {...copyFeedback} />
      </div>
    ) : "未记录错误信息")],
    ["请求 ID", request.requestId && copyValue(request.requestId, "请求 ID")],
    ["请求时间", requestTime(request)],
    ["计时说明", timing],
    ["结束原因", stop],
    ["客户端", client],
    ["项目", request.project && copyValue(request.project, "项目路径", shortPath(request.project))],
  ] as const).filter(([, value]) => Boolean(value));

  return (
    <div className="request-detail">
      {fields.length > 0
        ? <dl>{fields.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
        : <p>此请求没有其他记录</p>}
    </div>
  );
}
