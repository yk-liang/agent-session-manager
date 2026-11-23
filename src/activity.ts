import type { BrowseOp, ContentBlock, ToolOp } from "./types";
import { toolStatus, type SessionBlock } from "./sessionContent.ts";
import { shortPath } from "./utils.ts";

/** 一次工具调用可包含多个操作（如一个补丁改多个文件），每个操作单独成卡。 */
export type ToolItem = { key: string; part: SessionBlock; op?: ToolOp; index: number };
export type ActivityItem =
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
        if (browseSteps(op) && itemStatus(item) !== "error") {
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

/** 退出码 1 的正常结果：调用记录已含结论时直接采用，否则用结果上的退出码和输出形态核对静态含义。 */
const exitOneEvidence = { no_match: "empty", differences: "diff" } as const;

export function commandOutcome({ part, op }: ToolItem) {
  if (op?.kind !== "shell" || isDenied(part.result)) return;
  if (op.outcome) return op.outcome;
  const { result } = part;
  if (op.exit_one && result?.exit_code === 1 && result.command_output_kind === exitOneEvidence[op.exit_one]) return op.exit_one;
}

export function itemStatus(item: ToolItem) {
  if (commandOutcome(item)) return "completed";
  const { part, op } = item;
  if (op?.kind === "shell" && op.exit_code !== undefined) {
    if (part.content.status === "error" || part.result?.status === "error") return "error";
    if (op.exit_code !== 0) return "error";
  }
  return toolStatus(part.content, part.result);
}

/** 失败原因取结果的首个非空行，去掉工具自带的标签。 */
export function errorSummary(result?: ContentBlock) {
  const content = result?.content;
  const texts: unknown[] = Array.isArray(content) ? content.map(part => part?.text) : [content];
  for (const text of [result?.error_summary, ...texts]) {
    if (typeof text !== "string") continue;
    for (const line of text.slice(0, 4096).split("\n")) {
      const summary = line.replace(/^\s*<\/?[\w-]+>\s*/, "").trim();
      if (summary) return summary.slice(0, 240);
    }
  }
}

/** 拒绝执行时的固定文案（Claude Code 系的权限与分类器、Antigravity 的沙箱检查）：被拒绝的调用没有执行。 */
const DENIAL = /^(?:Permission to use .+?\bdenied\b|Permission for this action was denied|The user doesn't want to proceed with this tool use|permission check failed for unsandboxed|hook denied)|was blocked by a deny rule/i;

export function isDenied(result?: ContentBlock) {
  const summary = errorSummary(result);
  return !!summary && DENIAL.test(summary);
}

/** 项目内的路径显示为相对路径，其余缩写用户目录。 */
export function displayPath(path: string, cwd?: string | null) {
  const root = cwd?.replace(/\/$/, "");
  if (root && path === root) return ".";
  if (root && path.startsWith(`${root}/`)) return path.slice(root.length + 1);
  return shortPath(path);
}

export function splitPath(path: string) {
  const slash = path.lastIndexOf("/", path.length - 2);
  return slash < 0 ? { dir: "", name: path } : { dir: path.slice(0, slash), name: path.slice(slash + 1) };
}

/** 卡片标题里的长路径只保留末尾放得下的几级，前面以 … 代替。 */
export function compactPath(path: string, limit = 32) {
  if (path.length <= limit) return path;
  const parts = path.replace(/\/$/, "").split("/");
  for (let start = 1; start < parts.length - 1; start++) {
    const label = ["…", ...parts.slice(start)].join("/");
    if (label.length <= limit) return label;
  }
  return `…/${parts[parts.length - 1]}`;
}

/** 命令标题里的路径：项目内转为相对路径，其余长路径只保留末尾几级。 */
export function titlePaths(text: string, cwd?: string | null) {
  const root = cwd?.replace(/\/$/, "");
  return text.replace(/(?<![\w.~:/-])(?:~\/|\/)[^\s'"|;&<>()]+/g, path => {
    const relative = root && path.startsWith(`${root}/`) ? path.slice(root.length + 1) : shortPath(path);
    return compactPath(relative);
  });
}
