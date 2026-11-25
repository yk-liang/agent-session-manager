import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { errorSummary, itemStatus, type ToolItem } from "../activity";
import { compactPath, displayPath, splitPath } from "../utils";
import Collapsible from "./Collapsible";
import { ExpandableFrame } from "./ToolContent";
import UiIcon from "./UiIcon";

export type Tone = "dim" | "success" | "warning" | "danger" | "violet";

const statusLabels = new Map<string, string>([
  ["pending", "等待中"], ["running", "进行中"], ["error", "失败"], ["denied", "已拒绝"], ["cancelled", "已取消"],
]);

/** 完成是常态，只标出等待、进行中、失败、已拒绝和已取消；原因放在悬停提示里。 */
export function Status({ status, exit, reason }: { status?: string; exit?: number; reason?: string }) {
  const label = status ? statusLabels.get(status) : undefined;
  return label ? <span className={`tool-status ${status}`} title={reason}>
    {label}{status === "error" && exit !== undefined ? ` exit ${exit}` : ""}
  </span> : null;
}

/** 文件名突出、目录缩写，悬停显示完整路径。 */
export function PathName({ path, cwd }: { path: string; cwd: string | null }) {
  const { dir, name } = splitPath(displayPath(path, cwd));
  return <>
    <code className="op-name" title={path}>{name}</code>
    {dir && <span className="op-dir" title={path}>{compactPath(dir)}</span>}
  </>;
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
export function Stats({ items }: { items: Badges }) {
  const shown = [...new Set(items.filter((text): text is string => !!text))];
  return shown.length ? <span className="op-meta">{shown.map(text => <Chip key={text} tone="sans">{text}</Chip>)}</span> : null;
}

/** 卡片标题：类型图标和标题行内容。组合卡片借用同类卡片的标题时，用 `Badges` 追加自己的标签。 */
export type Head = { icon: LucideIcon; label: string; tone?: Tone; head: ReactNode };
export type Badges = (string | false | undefined)[];

export type StepRow = { icon: LucideIcon; label: string; text: ReactNode; title?: string; detail?: string; pattern?: boolean };

/** 多步调用的「步骤」：每步一行，依次是图标、动作、目标和补充说明。 */
export function StepList({ rows }: { rows: StepRow[] }) {
  return <ExpandableFrame lines={rows.length} label="步骤">{() => <ol className="op-steps">{rows.map((row, i) =>
    <li key={i} title={row.title}>
      <UiIcon icon={row.icon} size={12} /><span className="op-step-label">{row.label}</span>
      <code className={row.pattern ? "pattern" : undefined}>{row.text}</code>
      {row.detail && <span className="op-step-detail">{row.detail}</span>}
    </li>)}</ol>}</ExpandableFrame>;
}

/** 工具卡片外框：标题始终一行，内容区可收缩，右侧的状态和工具名固定。 */
export function OpCard({ item, className, icon, label, tone, head, children }: {
  item: ToolItem;
  className: string;
  icon: LucideIcon;
  label: string;
  tone?: Tone;
  head: ReactNode;
  children: ReactNode;
}) {
  const { result } = item.part;
  const status = itemStatus(item);
  const failed = status === "error" || status === "denied";
  const reason = failed || status === "cancelled" ? errorSummary(result) : undefined;
  const exitLine = reason?.match(/^(?:Exit code\s*[:：]?|Process exited with code|退出码\s*[:：]?)\s*(-?\d+)$/i);
  const recordedExit = item.op?.kind === "shell" ? item.op.exit_code : undefined;
  const exit = recordedExit || result?.exit_code || (exitLine && Number(exitLine[1])) || undefined;
  const tool = item.part.content.name;
  return <Collapsible id={item.key} className={`tool op ${className}${failed ? " error" : ""}`}
    head={<>
      <TypeIcon icon={icon} label={label} tone={tone} />
      <span className="op-main">{head}</span>
      <span className="collapsible-status">
        <Status status={status} exit={exit} reason={exitLine ? undefined : reason} />
        {tool && <span className="tool-tag" title={tool}>{tool}</span>}
      </span>
    </>}>
    {children}
  </Collapsible>;
}
