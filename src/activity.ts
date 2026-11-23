import type { BrowseOp, ContentBlock, ToolOp } from "./types";
import { toolStatus, type SessionBlock } from "./sessionContent.ts";
import { fitPath, shortPath } from "./utils.ts";

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
        if (browseSteps(op)) run.push(item);
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

/** 命令以退出码为准；`grep`、`rg` 没有匹配时退出码为 1，不算失败。 */
export function itemStatus({ part, op }: ToolItem) {
  if (op?.kind === "shell" && op.exit_code !== undefined) {
    if (part.content.status === "error" || part.result?.status === "error") return "error";
    if (op.no_match) return "completed";
    if (op.exit_code !== 0) return "error";
  }
  return toolStatus(part.content, part.result);
}

/** 失败原因取结果的首个非空行，去掉工具自带的标签。 */
export function errorSummary(result?: ContentBlock) {
  const content = result?.content;
  const text = typeof content === "string" ? content
    : Array.isArray(content) ? content.find(part => typeof part?.text === "string")?.text : undefined;
  return typeof text === "string"
    ? text.replace(/<\/?[\w-]+>/g, "").split("\n").map(line => line.trim()).find(Boolean)?.slice(0, 240)
    : undefined;
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

export function compactPath(path: string, limit = 32) {
  return fitPath(path, label => label.length <= limit);
}

/** 命令标题里的路径：项目内转为相对路径，其余长路径按 fish 风格缩写。 */
export function titlePaths(text: string, cwd?: string | null) {
  const root = cwd?.replace(/\/$/, "");
  const relative = root ? text.split(`${root}/`).join("") : text;
  return relative.replace(/(?<![\w.~:/-])(?:~|\/)[^\s'"|;&<>()]{24,}/g, path => compactPath(shortPath(path)));
}
