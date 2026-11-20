import type { RequestEvent, RequestMetadata } from "./types";
import { formatDateTime } from "./format";
import { requestErrorText } from "./metrics";
import CopyButton from "../components/CopyButton";

const routineStops = new Set(["end_turn", "stop", "tool_use", "tool-calls"]);

function requestTime({ startedAt, finishedAt }: RequestMetadata) {
  const format = (time: number) => `${formatDateTime(time)}.${String(time % 1000).padStart(3, "0")}`;
  if (startedAt && finishedAt) return `${format(startedAt)} – ${format(finishedAt)}`;
  if (startedAt) return `开始 ${format(startedAt)}`;
  if (finishedAt) return `结束 ${format(finishedAt)}`;
}

export default function RequestDetail({ event, client, session, project, copyFeedback }: {
  event: RequestEvent;
  client: string;
  session: string;
  project: string | null;
  copyFeedback: { onCopied: () => void; onError: () => void };
}) {
  const { request } = event;
  const failed = event.status === "failed";
  const error = requestErrorText(request);
  const timing = [
    request.timing === "codex_boundary" && request.latencyMs != null && "耗时根据日志估算",
    request.ttftScope === "turn" && request.ttftMs != null && "首字延迟按整轮对话计算",
  ].filter(Boolean).join("，");
  const stop = request.stop && !routineStops.has(request.stop) ? request.stop : undefined;
  const parentSession = request.parentSessionId !== session ? request.parentSessionId : undefined;
  const copyValue = (value: string, label: string) => (
    <div className="request-detail-copy">
      <code>{value}</code>
      <CopyButton className="btn-icon btn-ghost" value={value} label={`复制${label}`} {...copyFeedback} />
    </div>
  );
  const fields = ([
    ["错误信息", failed && (error ? (
      <div className="request-detail-copy request-failure-detail" role="region" aria-label="错误信息">
        <pre>{error}</pre>
        <CopyButton className="btn-icon btn-ghost" value={error} label="复制错误信息" {...copyFeedback} />
      </div>
    ) : "未记录错误信息")],
    ["请求 ID", request.requestId && copyValue(request.requestId, "请求 ID")],
    ["会话", session ? copyValue(session, "会话 ID") : <span className="request-missing">未记录</span>],
    ["类型", request.kind || <span className="request-missing">未记录</span>],
    ["父会话", parentSession && copyValue(parentSession, "父会话 ID")],
    ["请求时间", requestTime(request)],
    ["计时说明", timing],
    ["结束原因", stop],
    ["客户端", client],
    ["项目", project && copyValue(project, "项目路径")],
  ] as const).filter(([, value]) => Boolean(value));

  return (
    <dl className="request-detail">
      {fields.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}
    </dl>
  );
}
