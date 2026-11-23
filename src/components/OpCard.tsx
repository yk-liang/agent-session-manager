import { useContext, useState, type ReactNode } from "react";
import { Folder, SquareCode, type LucideIcon } from "lucide-react";
import { compactPath, displayPath, errorSummary, itemStatus, type ToolItem } from "../activity";
import type { ContentBlock, ShellOp, ToolOp } from "../types";
import { shortPath } from "../utils";
import Collapsible from "./Collapsible";
import { DetailState } from "./DetailState";
import { Payload, ToolInput, ToolResult, ToolSection } from "./ToolPayload";
import UiIcon from "./UiIcon";

export type Tone = "dim" | "accent" | "success" | "warning" | "danger" | "violet";

const statusLabels: Record<string, string> = { pending: "等待中", running: "进行中", error: "失败" };

/** 完成是常态，只标出等待、进行中和失败。 */
export function Status({ status }: { status?: string }) {
  return status && statusLabels[status] ? <span className={`tool-status ${status}`}>{statusLabels[status]}</span> : null;
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

/** 命令在别的目录执行时标出目录。 */
export function CwdTag({ dir }: { dir?: string }) {
  const { cwd } = useContext(DetailState);
  if (!dir || (cwd && (dir === cwd || dir === shortPath(cwd) || displayPath(dir, cwd) === "."))) return null;
  return <span className="op-cwd" title={`在 ${dir} 中执行`}><UiIcon icon={Folder} size={12} />{compactPath(displayPath(dir, cwd), 28)}</span>;
}

/** 工具卡片外框：图标、标题、状态和工具名；命令失败标出退出码，其余失败把原因显示在标题下方。 */
export function OpCard({ item, className, icon, label, tone, tag, open = false, head, children }: {
  item: ToolItem;
  className: string;
  icon: LucideIcon;
  label: string;
  tone?: Tone;
  tag?: string;
  open?: boolean;
  head: ReactNode;
  children: ReactNode;
}) {
  const status = itemStatus(item);
  const failed = status === "error";
  const exit = failed && item.op?.kind === "shell" ? item.op.exit_code : undefined;
  const reason = failed && !exit ? errorSummary(item.part.result) : undefined;
  const shownTag = tag ?? item.part.content.name;
  const scriptResult = item.index === 0 ? item.part.content.script_result : undefined;
  return <Collapsible id={item.key} className={`tool op ${className}${failed ? " error" : ""}`} defaultOpen={open && !failed}
    head={<>
      <TypeIcon icon={icon} label={label} tone={tone} />
      {head}
      <span className="collapsible-status">
        {exit ? <span className="tool-status error">退出码 {exit}</span> : <Status status={status} />}
        {shownTag && <span className="tool-tag">{shownTag}</span>}
      </span>
    </>}
    note={reason && <div className="op-error" title={reason}>{reason}</div>}>
    {children}
    {scriptResult && <Collapsible id={`${item.key}.script`} className="tool"
      head={<><TypeIcon icon={SquareCode} label="脚本" /><span className="op-title">脚本输出</span></>}>
      <Payload block={scriptResult} kind="output">{loaded => <ToolResult result={loaded} start={null} />}</Payload>
    </Collapsible>}
  </Collapsible>;
}

/** 卡片正文的完整操作描述：延迟加载的调用在展开时读取。 */
export function FullOp<T extends ToolOp>({ item, actions, children }: { item: ToolItem; actions?: ReactNode; children: (op: T, call: ContentBlock) => ReactNode }) {
  return <Payload block={item.part.content} kind="input" actions={actions}>
    {loaded => children((loaded.ops?.[item.index] ?? item.op) as T, loaded)}
  </Payload>;
}

/** 原始输入与结果；命令按 shell 形式展示，输出不编号。 */
export function ToolBody({ item, actions }: { item: ToolItem; actions?: ReactNode }) {
  const { part } = item;
  const failed = itemStatus(item) === "error";
  const shell = item.op?.kind === "shell";
  return <>
    <Payload block={part.content} kind="input" actions={actions}>{loaded => <ToolInput value={loaded.input} actions={actions}
      command={shell ? (loaded.ops?.[item.index] as ShellOp | undefined)?.command : undefined} />}</Payload>
    {part.result && <Payload block={part.result} kind={failed ? "error" : "output"}>
      {loaded => <ToolResult result={loaded} failed={failed} start={shell ? null : 1} />}
    </Payload>}
  </>;
}

/** 语义视图与原始数据之间切换；按钮放在正文第一节的操作区。 */
export function useRawView() {
  const [raw, setRaw] = useState(false);
  const toggle = <button type="button" className="content-action btn-ghost btn-sm" aria-pressed={raw} onClick={() => setRaw(!raw)}>原始数据</button>;
  return { raw, toggle };
}

/** 结果正文；尚无结果时说明在等待。 */
export function ResultOutput({ item, title, start = null, actions }: { item: ToolItem; title?: string; start?: number | null; actions?: ReactNode }) {
  const { result } = item.part;
  const status = itemStatus(item);
  const failed = status === "error";
  if (!result) {
    return <ToolSection kind={failed ? "error" : "output"} title={title} actions={actions}><p className="tool-loading tool-payload-message">
      {status === "completed" || failed ? "无输出" : "等待结果"}
    </p></ToolSection>;
  }
  return <Payload block={result} kind={failed ? "error" : "output"} actions={actions}>
    {loaded => <ToolResult result={loaded} failed={failed} title={title} start={start} actions={actions} />}
  </Payload>;
}
