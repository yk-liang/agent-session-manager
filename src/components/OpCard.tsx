import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { errorSummary, isDenied, itemStatus, type ToolItem } from "../activity";
import Collapsible from "./Collapsible";
import UiIcon from "./UiIcon";

export type Tone = "dim" | "accent" | "success" | "warning" | "danger" | "violet";

const statusLabels: Record<string, string> = { pending: "等待中", running: "进行中", error: "失败", denied: "已拒绝" };

/** 完成是常态，只标出等待、进行中、失败和被拒绝；失败原因放在悬停提示里。 */
export function Status({ status, exit, reason }: { status?: string; exit?: number; reason?: string }) {
  return status && statusLabels[status] ? <span className={`tool-status ${status}`} title={reason}>
    {statusLabels[status]}{status === "error" && exit !== undefined ? ` exit ${exit}` : ""}
  </span> : null;
}
export function TypeIcon({ icon, label, tone }: { icon: LucideIcon; label: string; tone?: Tone }) {
  return <span className={`collapsible-type op-icon${tone ? ` ${tone}` : ""}`} title={label}>
    <UiIcon icon={icon} size={16} />
    <span className="sr-only">{label}</span>
  </span>;
}

export function Chip({ children, tone }: { children: ReactNode; tone?: "created" | "removed" | "answer" | "kind" | "sans" }) {
  return <span className={`op-chip${tone ? ` ${tone}` : ""}`} title={typeof children === "string" ? children : undefined}>{children}</span>;
}

/** 标题里的统计标签（步数、行数、匹配数）：短且完整显示，空值不渲染。 */
export function Stats({ items }: { items: (string | false | undefined)[] }) {
  const shown = items.filter((text): text is string => !!text);
  return shown.length ? <span className="op-meta">{shown.map(text => <Chip key={text} tone="sans">{text}</Chip>)}</span> : null;
}

/** 工具卡片外框：标题始终一行，内容区可收缩，右侧的状态和工具名固定。 */
export function OpCard({ item, className, icon, label, tone, open = false, head, children }: {
  item: ToolItem;
  className: string;
  icon: LucideIcon;
  label: string;
  tone?: Tone;
  open?: boolean;
  head: ReactNode;
  children: ReactNode;
}) {
  const status = itemStatus(item);
  const failed = status === "error";
  const { result } = item.part;
  const reason = failed ? errorSummary(result) : undefined;
  const exitLine = reason?.match(/^(?:Exit code\s*[:：]?|Process exited with code|退出码\s*[:：]?)\s*(-?\d+)$/i);
  const recordedExit = item.op?.kind === "shell" ? item.op.exit_code : undefined;
  const exit = recordedExit || result?.exit_code || (exitLine && Number(exitLine[1])) || undefined;
  const tool = item.part.content.name;
  return <Collapsible id={item.key} className={`tool op ${className}${failed ? " error" : ""}`} defaultOpen={open && !failed}
    head={<>
      <TypeIcon icon={icon} label={label} tone={tone} />
      <span className="op-main">{head}</span>
      <span className="collapsible-status">
        <Status status={failed && isDenied(result) ? "denied" : status} exit={exit} reason={exitLine ? undefined : reason} />
        {tool && <span className="tool-tag" title={tool}>{tool}</span>}
      </span>
    </>}>
    {children}
  </Collapsible>;
}
