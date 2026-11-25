import { useContext } from "react";
import { FilePen, Globe, Image, Info, ListTodo, Plug, SquareCode, SquareTerminal, Wrench, type LucideIcon } from "lucide-react";
import type { ToolItem } from "../activity";
import type { ChangeOp, ExecCall, ShellOp, ToolOp } from "../types";
import { displayPath, titlePaths } from "../utils";
import { actionRow, commandHead, intents, scriptHead } from "./CommandCards";
import { DetailState } from "./DetailState";
import DiffView from "./DiffView";
import { browseHead, browseRow, changeHead, changeRow, changeView } from "./FileCards";
import { OpCard, Stats, StepList, type Badges, type Head, type StepRow } from "./OpCard";
import { diffText, scriptView } from "./ResultViews";
import { isSemanticOp, semanticView } from "./SemanticCards";
import { commandsView, ShellText, textView, type View } from "./ToolContent";
import { ToolSections } from "./ToolSections";
import { webFetchHead, webSearchesView, webSearchHead } from "./WebCards";

type ExecOp = Extract<ToolOp, { kind: "exec" }>;

const fallbacks = new Map<string, [LucideIcon, string]>([
  ["exec_command", [SquareTerminal, "运行命令"]], ["apply_patch", [FilePen, "应用补丁"]], ["view_image", [Image, "查看图片"]],
  ["write_stdin", [SquareTerminal, "访问后台会话"]], ["update_plan", [ListTodo, "更新计划"]], ["clock__curr_time", [Info, "查看当前时间"]],
  ["web__run", [Globe, "网页工具"]],
]);

/** 参数求不出时只知道调用了哪个工具。 */
function fallback(call: ExecCall): [LucideIcon, string] {
  return fallbacks.get(call.tool) ?? (call.tool.startsWith("mcp__") ? [Plug, call.tool.split("__").slice(1).join(" / ")] : [Wrench, call.tool]);
}

/** 主动作的标题与同类工具单独调用时相同，`badges` 换成整段脚本的结构和步数。 */
function leadHead(op: ToolOp, tool: string, cwd: string | null, badges: Badges): Head {
  switch (op.kind) {
    case "shell": return op.steps?.length ? browseHead(op.steps, cwd, badges) : commandHead(op, cwd, op.description, badges);
    case "read": case "search": case "find": case "list": return browseHead([op], cwd, badges);
    case "edit": case "write": case "delete": return changeHead(op, cwd, badges);
    case "script": return scriptHead(op, badges);
    case "web_search": return webSearchHead(op, badges);
    case "web_fetch": return webFetchHead(op, !!op.find, badges);
    case "skill": case "exec": return { icon: Wrench, label: tool, head: <Stats items={badges} /> };
    default: {
      const view = semanticView(op, cwd, tool);
      return { ...view, head: <>{view.head}<Stats items={badges} /></> };
    }
  }
}

function execHead(op: ExecOp, cwd: string | null, badges: Badges): Head {
  const call = op.lead && op.calls[op.lead[0]];
  const lead = op.lead && call?.ops?.[op.lead[1]];
  if (call && lead) return leadHead(lead, call.tool, call.cwd ?? cwd, badges);
  if (call) {
    const [icon, label] = fallback(call);
    const title = call.target ?? label;
    return { icon, label, head: <><span className="op-title" title={title}>{title}</span><Stats items={badges} /></> };
  }
  const features = new Set(op.features);
  const title = features.has("invalid") ? "JavaScript 语法异常" : features.has("state") ? "读写脚本状态" : undefined;
  return { icon: SquareCode, label: "JavaScript", head: <>
    {title && <span className="op-title">{title}</span>}<Stats items={[`JavaScript ${op.lines} 行`, ...badges]} />
  </> };
}

function opRows(op: ToolOp, tool: string, cwd: string | null): StepRow[] {
  switch (op.kind) {
    case "shell": {
      if (op.steps?.length) return op.steps.map(step => browseRow(step, cwd));
      if (op.actions?.length) return op.actions.map(actionRow);
      const [icon, label] = intents[op.intent];
      return [{ icon, label, text: <ShellText command={titlePaths(op.head || op.command?.split("\n")[0] || "", cwd)} />, title: op.command }];
    }
    case "read": case "search": case "find": case "list": return [browseRow(op, cwd)];
    case "edit": case "write": case "delete": return [changeRow(op, cwd)];
    case "script": return [{ icon: SquareCode, label: "脚本", text: op.title ?? op.first }];
    case "web_search": return [{ icon: Globe, label: "网页搜索", text: op.queries.join(" · ") }];
    case "web_fetch": return [{ icon: Globe, label: op.find ? "在网页中查找" : "读取网页", text: op.find ? op.prompt ?? op.url : op.url, detail: op.find ? op.url : op.prompt }];
    case "skill": case "exec": return [{ icon: Wrench, label: tool, text: "" }];
    default: {
      const { icon, label, summary } = semanticView(op, cwd, tool);
      return [{ icon, label, text: summary }];
    }
  }
}

function callRows(call: ExecCall, cwd: string | null): StepRow[] {
  if (call.ops?.length) return call.ops.flatMap(op => opRows(op, call.tool, cwd));
  const [icon, label] = fallback(call);
  return [{ icon, label, text: call.target ?? call.expression ?? "", title: call.expression }];
}

function stepCount(call: ExecCall) {
  return call.ops?.reduce((sum, op) => sum + (op.kind === "shell" ? op.steps?.length || (op.more ?? 0) + 1 : 1), 0) || 1;
}

function changesView(changes: ChangeOp[], cwd: string | null): View | undefined {
  if (changes.length < 2) return changes[0] && changeView(changes[0]);
  return { label: "改动", body: <DiffView files={changes.map(change => ({
    path: `${displayPath(change.path, cwd)}${change.kind === "delete" ? "（删除）" : ""}`,
    hunks: change.kind === "edit" ? change.hunks ?? []
      : change.kind === "write" ? [{ old_start: null, new_start: 1, lines: (change.content ?? "").split("\n").map(line => `+${line}`) }] : [],
  }))} /> };
}

function queryView(calls: ExecCall[]): View | undefined {
  const searches = calls.flatMap(call => (call.ops ?? [])
    .filter((op): op is Extract<ToolOp, { kind: "web_search" }> => op.kind === "web_search")
    .map(search => ({ tool: call.tool, queries: search.queries, input: call.input })));
  return webSearchesView(searches);
}

function scriptsView(ops: ToolOp[]): View | undefined {
  const scripts = ops.flatMap(op => op.kind === "shell" && op.script?.body !== undefined
    ? [{ lang: op.script.lang ?? "标准输入", body: op.script.body }]
    : op.kind === "script" && op.body !== undefined
      ? [{ lang: op.lang ?? "脚本", body: op.body }]
      : []);
  if (!scripts.length) return;
  const label = (lang: string) => lang === "JavaScript" ? "内嵌脚本" : lang;
  if (scripts.length === 1) return textView(label(scripts[0].lang), scripts[0].body);
  const langs = new Set(scripts.map(script => script.lang));
  return textView(langs.size === 1 ? label(scripts[0].lang) : "脚本", scripts.map(script => script.body).join("\n\n"), null);
}

/** 补上语义工具（如 MCP、任务清单、提问等）的结构化输入视图。 */
function nativeViews(op: ToolOp, call: ExecCall, cwd: string | null): View[] {
  if (!isSemanticOp(op)) return [];
  const view = semanticView(op, cwd, call.tool);
  return [view.preview, ...(view.input?.(op, { type: "tool_use", input: call.input }) ?? [])]
    .filter((item): item is View => !!item);
}

function detailsViews(calls: ExecCall[], cwd: string | null): View[] {
  const groups = calls.flatMap(call => (call.ops ?? []).flatMap(op => {
    const views = nativeViews(op, call, call.cwd ?? cwd);
    return views.length ? [{ tool: call.tool, views }] : [];
  }));
  if (!groups.length) return [];
  if (groups.length === 1) return groups[0].views;
  return [{ label: "参数", body: <div className="exec-call-details">{groups.map(({ tool, views }, i) => <div key={i}>
    <div className="exec-call-detail-title">{tool}</div>
    {views.map((view, j) => <div key={j}>{view.body}</div>)}
  </div>)}</div> }];
}

function inputViews(op: ExecOp, cwd: string | null): (View | false | undefined)[] {
  const ops = op.calls.flatMap(call => call.ops ?? []);
  const rows = op.calls.flatMap(call => callRows(call, call.cwd ?? cwd));
  const shells = op.calls.flatMap(call => (call.ops ?? []).filter((op): op is ShellOp => op.kind === "shell")
    .map(shell => ({ shell, cwd: call.cwd })));
  const features = new Set(op.features);
  const copyCommands = !features.has("conditional") && !features.has("loop") && !features.has("invalid")
    && op.calls.every(call => !call.expression && (!["exec_command", "shell", "container.exec"].includes(call.tool) || !!call.ops?.some(op => op.kind === "shell")));
  const script = scriptsView(ops);
  const steps = rows.length > 1 && { label: "步骤", body: <StepList rows={rows} /> };
  const commands = commandsView(shells.flatMap(({ shell, cwd }) => shell.command ? [{ command: shell.command, cwd }] : []), cwd, copyCommands);
  const changes = changesView(ops.filter((op): op is ChangeOp => op.kind === "edit" || op.kind === "write" || op.kind === "delete"), cwd);
  const leadOp = op.lead ? op.calls[op.lead[0]]?.ops?.[op.lead[1]] : undefined;
  const leadScript = !!script && (leadOp?.kind === "script" || leadOp?.kind === "shell" && leadOp.intent === "script");
  const details = detailsViews(op.calls, cwd);
  return leadScript ? [script, steps, commands, changes, queryView(op.calls), ...details] : [steps, script, commands, changes, queryView(op.calls), ...details];
}

/** Codex 的 `exec`：JavaScript 编排的一组工具调用，按步骤、命令等展示，与这些工具单独调用时一致。 */
export function ExecCard({ item, op }: { item: ToolItem; op: ExecOp }) {
  const { cwd } = useContext(DetailState);
  const features = new Set(op.features);
  const steps = op.calls.reduce((sum, call) => sum + stepCount(call), 0);
  const { icon, label, tone, head } = execHead(op, cwd, [
    features.has("parallel") && "并行", features.has("loop") && "循环", features.has("conditional") && "含条件", steps > 1 && `+${steps - 1} 步`,
  ]);
  const ops = op.calls.flatMap(call => call.ops ?? []);
  const notes = new Set(ops.map(op => op.kind === "shell" ? op.description : undefined).filter(Boolean));
  // 只改文件的脚本没有实际输出时隐藏输出小节；失败、脚本打印消息或保存输出文件时仍保留。
  const patchOnly = ops.length > 0 && op.calls.every(call => call.ops?.length) && ops.every(op => ["edit", "write", "delete"].includes(op.kind));
  return <OpCard item={item} className="exec" icon={icon} label={label} tone={tone} head={head}>
    <ToolSections<ExecOp> item={item} input={{ sourceLabel: "JavaScript", note: [...notes].join(" · "), views: full => inputViews(full, cwd) }}
      output={{ hideEmpty: patchOnly, terminal: true, views: result => {
        const decoded = scriptView(result, "结果", true);
        return [decoded?.plain && diffText(decoded.copy), decoded];
      } }} />
  </OpCard>;
}
