import type { BrowseOp, ContentBlock, ToolOp } from "./types";
import type { SessionBlock } from "./sessionContent";

/** 一次工具调用可包含多个操作（如一个补丁改多个文件），每个操作单独成卡。 */
export type ToolItem = { key: string; part: SessionBlock; op?: ToolOp; index: number };
type ActivityItem =
  | { type: "thinking"; key: string; part: SessionBlock }
  | { type: "result"; key: string; part: SessionBlock }
  | { type: "tool"; item: ToolItem }
  | { type: "browse"; key: string; items: ToolItem[] };

/** 读取、搜索、查找、列目录，以及只做这些事的命令。 */
export function browseSteps(op?: ToolOp): BrowseOp[] | undefined {
  if (!op) return;
  if (op.kind === "read" || op.kind === "search" || op.kind === "find" || op.kind === "list") return [op];
  if (op.kind === "shell" && op.steps?.length) return op.steps;
}

/** 相邻的浏览操作合并为一组，至少两次才合并。 */
export function activityItems(parts: SessionBlock[]): ActivityItem[] {
  const items: ActivityItem[] = [];
  let run: ToolItem[] = [];
  const flush = () => {
    if (run.length > 1) items.push({ type: "browse", key: `${run[0].key}:browse`, items: run });
    else items.push(...run.map(item => ({ type: "tool" as const, item })));
    run = [];
  };
  for (const part of parts) {
    const { content } = part;
    if (content.type === "tool_use") {
      const ops: (ToolOp | undefined)[] = content.ops?.length ? content.ops : [undefined];
      ops.forEach((op, i) => {
        const item = { key: ops.length > 1 ? `${part.key}.${i}` : part.key, part, op, index: i };
        const status = itemStatus(item);
        if (browseSteps(op) && status !== "error" && status !== "denied" && status !== "cancelled") {
          if (run.length === 20) flush();
          run.push(item);
        }
        else {
          flush();
          items.push({ type: "tool", item });
        }
      });
      continue;
    }
    flush();
    items.push({ type: content.type === "thinking" ? "thinking" : "result", key: part.key, part });
  }
  flush();
  return items;
}

export type ToolStatus = NonNullable<ContentBlock["status"]> | "denied";

/** 退出码 1 的正常结果：调用记录已含结论时直接采用，否则用结果上的退出码和输出形态核对静态含义。 */
export function commandOutcome({ part, op }: ToolItem) {
  if (op?.kind !== "shell" || skipped(part.result)) return;
  if (op.outcome) return op.outcome;
  const { result } = part;
  if (op.exit_one && result?.exit_code === 1) {
    if (op.exit_one === "no_match" && result.command_output_kind === "empty") return "no_match";
    if (op.exit_one === "differences" && result.command_output_kind === "diff") return "differences";
    if (op.exit_one === "silent_differences" && result.command_output_kind === "empty") return "differences";
  }
}

export function resultStatus(result?: ContentBlock): ToolStatus | undefined {
  if (!result) return;
  if (result.status === "cancelled") return "cancelled";
  if (result.status === "error" || result.is_error) return skipped(result) ?? "error";
  return result.status;
}

export function itemStatus(item: ToolItem): ToolStatus | undefined {
  const { content, result } = item.part;
  if (content.status === "cancelled" || result?.status === "cancelled") return "cancelled";
  if (commandOutcome(item)) return "completed";
  const shellFailed = item.op?.kind === "shell" && item.op.exit_code !== undefined && item.op.exit_code !== 0;
  if (content.status === "error" || result?.status === "error" || result?.is_error || shellFailed) {
    return skipped(result) ?? "error";
  }
  if (content.status === "completed" || result?.status === "completed") return "completed";
  return result?.status ?? content.status ?? (result && "completed");
}

function stripTag(text: string) {
  return text.replace(/^<\/?[\w-]+>\s*/, "").trim();
}

function fullContentText(result?: ContentBlock) {
  const content = result?.content;
  if (typeof content === "string") return content.trim() ? content.slice(0, 4096).trim() : undefined;
  if (!Array.isArray(content)) return;
  const parts = content.flatMap(part => typeof part?.text === "string" && part.text.trim() ? [part.text.trim()] : []);
  return parts.length ? parts.join("\n\n").slice(0, 4096) : undefined;
}

function summaryText(result?: ContentBlock) {
  return typeof result?.error_summary === "string" && result.error_summary.trim()
    ? result.error_summary.slice(0, 4096).trim()
    : undefined;
}

/** 失败原因：优先用后端摘要，否则取结果正文的首个非空行。 */
export function errorSummary(result?: ContentBlock) {
  const summary = summaryText(result);
  if (summary) return summary.slice(0, 240);
  const full = fullContentText(result);
  return full ? stripTag(full).split("\n").find(line => line.trim())?.trim().slice(0, 240) : undefined;
}

/** 没有执行的调用：被权限、自动模式分类器、安全检查或钩子拒绝，或被用户取消。 */
const DENIED_TAG = /^<tool_use_error>\s*Blocked:/i;
const DENIED = /^(?:Permission (?:to use|for this)[\s\S]*?\bdenied\b|The user doesn't want to proceed with this tool use|permission check failed for|hook denied|tool call denied by pre-tool hook)|was blocked by a deny rule|denied by (?:a )?pre-tool hook/i;
const CANCELLED = /^(?:工具执行已取消(?:\n|$)|(?:Command|Operation|Tool(?: execution| call)?|Web search|Search|Request|Execution) (?:aborted|cancell?ed)\.?)$/i;

export function skipped(result?: ContentBlock): "denied" | "cancelled" | undefined {
  if (result?.status === "cancelled") return "cancelled";
  const full = fullContentText(result);
  const raw = full ?? summaryText(result);
  if (!raw) return;
  if (DENIED_TAG.test(raw)) return "denied";
  if (DENIED.test(stripTag(raw))) return "denied";
  // 整段匹配取消提示需要完整正文，避免把截断的 error_summary 或多段错误误判为取消。
  if (full && CANCELLED.test(stripTag(full))) return "cancelled";
}
