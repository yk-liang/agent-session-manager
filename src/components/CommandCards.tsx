import { useContext } from "react";
import {
  Activity, AppWindow, Archive, Bot, Braces, CircleStop, Copy, Database, Download, ExternalLink, FilePen, FilePlus, FileSearchCorner, FileText, KeyRound, Link2, ShieldCheck,
  FlaskConical, FolderOpen, FolderPlus, GitBranch, GitCommitHorizontal, GitCompare, GitPullRequest, Globe, Hammer, Hash, History,
  Hourglass, Image, Info, ListTree, Move, Package, Plug, ScanSearch, Server, SquareCode, SquareTerminal, Trash, Wrench, type LucideIcon,
} from "lucide-react";
import { commandOutcome, compactPath, displayPath, splitPath, titlePaths, type ToolItem } from "../activity";
import type { ExecCall, ShellIntent, ShellOp, ToolOp } from "../types";
import { DetailState } from "./DetailState";
import { browseTarget, changeView } from "./FileCards";
import { OpCard, Stats, type Tone } from "./OpCard";
import { diffResult, diffText } from "./ResultViews";
import { commandView, ExpandableFrame, payloadText, plainResultText, scriptView, ShellText, textView } from "./ToolContent";
import { ToolSections } from "./ToolSections";
import UiIcon from "./UiIcon";
import { webScriptSearch, webSearchView } from "./WebCards";

const intents: Record<ShellIntent, [LucideIcon, string, Tone?]> = {
  read: [FileText, "读取文件", "dim"], search: [FileSearchCorner, "搜索", "dim"], find: [ScanSearch, "查找文件", "dim"],
  list: [FolderOpen, "列出目录", "dim"], write: [FilePlus, "写入文件", "success"], modify: [FilePen, "修改文件", "warning"],
  script: [SquareCode, "脚本"], run: [SquareTerminal, "运行程序"],
  diff: [GitCompare, "比较文件"], git_diff: [GitCompare, "Git 查看改动"], git_log: [History, "Git 历史"],
  git_commit: [GitCommitHorizontal, "Git 提交"], git_push: [GitBranch, "Git 推送"], git_change: [GitBranch, "更改 Git 状态", "warning"],
  git_read: [FileText, "读取 Git 历史文件", "dim"], git_stage: [GitBranch, "Git 暂存改动"],
  git: [GitBranch, "Git"], github: [GitPullRequest, "GitHub CLI"],
  test: [FlaskConical, "测试"], build: [Hammer, "构建与检查"], install: [Package, "安装依赖"],
  remote: [Server, "远程执行"], http: [Globe, "HTTP 请求"], download: [Download, "保存 HTTP 响应"], browser: [AppWindow, "浏览器自动化"], database: [Database, "数据库"],
  agent: [Bot, "调用 Agent"], agent_manage: [Bot, "管理 Agent"], inspect: [Info, "查看信息", "dim"], transform: [Braces, "转换内容"],
  mkdir: [FolderPlus, "创建目录"], remove: [Trash, "删除", "danger"], copy: [Copy, "复制"], move: [Move, "移动"],
  permissions: [KeyRound, "调整权限", "warning"], link: [Link2, "创建链接"],
  sign: [ShieldCheck, "调整代码签名", "warning"], archive: [Archive, "处理归档"],
  process: [Activity, "查看进程"], kill: [CircleStop, "结束进程"], wait: [Hourglass, "等待"], open: [ExternalLink, "打开"],
  count: [Hash, "统计", "dim"], other: [SquareTerminal, "命令"],
};

type ScriptOp = Extract<ToolOp, { kind: "script" }>;

function commandSteps(actions: ShellOp["actions"]) {
  if (!actions?.length) return;
  return { label: "步骤", body: <ExpandableFrame lines={actions.length} label="步骤">{() =>
    <ol className="op-steps">{actions.map(({ command, intent, target }, index) => {
      const [icon, label] = intents[intent];
      return <li key={index} title={target ?? command}>
        <UiIcon icon={icon} size={12} /><span className="command-step-label">{label}</span>
        <code><ShellText command={command} /></code>
      </li>;
    })}</ol>
  }</ExpandableFrame> };
}

const diffIntents = new Set<ShellIntent>(["diff", "git_diff", "git_log"]);

export function CommandCard({ item, op }: { item: ToolItem; op: ShellOp }) {
  const { cwd } = useContext(DetailState);
  const [icon, label, tone] = intents[op.intent];
  const file = op.target && ["write", "modify", "download"].includes(op.intent) ? splitPath(displayPath(op.target, cwd)) : undefined;
  const fileAction = op.intent === "modify" ? "原地修改" : op.intent === "download" ? "保存响应" : op.append ? "追加" : "写入";
  const more = op.more ? `+${op.more} 步` : undefined;
  const script = op.script && `${op.script.lang ? `${op.script.lang} ` : ""}${op.script.lines} 行`;
  const description = op.description?.trim() || item.part.content.description?.trim();
  const inline = op.intent === "script" ? op.script : undefined;
  const python = inline?.lang === "Python";
  const title = inline && (description || (!python && inline.first));
  const outcome = commandOutcome(item);
  const badges = [
    outcome === "differences" && "有差异",
    outcome === "no_match" && "无匹配",
    ...op.related?.slice(0, 2).map(intent => intents[intent][1]) ?? [],
    op.looped && "循环",
    more,
  ];
  return <OpCard item={item} className={`command ${op.intent}`} icon={icon} label={label} tone={tone}
    head={file ? <>
      <code className="op-name" title={op.target}>{file.name}</code>
      {file.dir && <span className="op-dir" title={op.target}>{compactPath(file.dir)}</span>}
      <Stats items={[op.script ? `${fileAction} ${op.script.lines} 行` : fileAction, ...badges]} />
    </> : <>
      {title ? <span className="op-title" title={title}>{title}</span>
        : !python && op.head && <code className="op-name cmd" title={op.head}><ShellText command={titlePaths(op.head, cwd)} /></code>}
      <Stats items={[script, ...badges]} />
    </>}>
    <ToolSections<ShellOp> item={item} input={{ note: [description, op.conditional && "含条件步骤"].filter(Boolean).join(" · "), views: full => {
      const script = full.script?.body !== undefined && textView(file ? "内容" : full.script.lang ?? "标准输入", full.script.body);
      const steps = commandSteps(full.actions);
      return op.intent === "script" || file && !full.conditional
        ? [script, steps, commandView(full.command)]
        : [steps, script, commandView(full.command)];
    } }} output={{ views: (result, full) => [
      (diffIntents.has(full.intent) || full.actions?.some(action => diffIntents.has(action.intent))) && diffResult(result),
    ] }} />
  </OpCard>;
}

type ExecOp = Extract<ToolOp, { kind: "exec" }>;

const webActions: Record<string, string> = {
  search_query: "网页搜索", open: "读取网页", find: "在网页中查找", image_query: "图片搜索", literature_search_query: "文献搜索",
};

function execTarget(call: ExecCall, cwd: string | null): [LucideIcon, string, string, string?] {
  switch (call.kind) {
    case "command": {
      const [icon, label] = intents[call.shell.intent];
      const first = call.shell.steps?.[0];
      const root = call.cwd ?? cwd;
      if (first) {
        const target = browseTarget(first, root);
        return [icon, label, target.name, target.dirTitle ? `${target.title ?? target.name}\n${target.dirTitle}` : target.title];
      }
      if (call.shell.target) return [icon, label, displayPath(call.shell.target, root), call.shell.target];
      return [icon, label, titlePaths(call.shell.head ?? "", root), call.shell.head];
    }
    case "patch": return [FilePen, "文件改动", call.changes.map(change => splitPath(change.path).name).join("、"), call.changes.map(change => change.path).join("\n")];
    case "image": return [Image, "查看图片", displayPath(call.path, cwd), call.path];
    case "tool": {
      const target = call.target ?? "";
      switch (call.name) {
        case "exec_command": return [SquareTerminal, "运行命令", target];
        case "apply_patch": return [FilePen, "应用补丁", target];
        case "update_plan": return [ListTree, "更新计划", target];
        case "view_image": return [Image, "查看图片", target];
        case "image_gen__imagegen": return [Image, "生成图片", target];
        case "clock__curr_time": return [Info, "查看当前时间", target];
        case "write_stdin": return [SquareTerminal, call.action === "poll" ? "读取后台输出" : call.action === "send" ? "发送会话输入" : "访问后台会话",
          target && `会话 ${target}`];
        case "web__run": return [Globe, webActions[call.action ?? ""] ?? "网页工具", target];
        case "search_openai_docs": case "mcp__openaiDeveloperDocs__search_openai_docs":
          return [Globe, "搜索 OpenAI 文档", target];
        default: return call.name.startsWith("mcp__")
          ? [Plug, "MCP", [call.name.split("__").slice(1).join(" / "), target].filter(Boolean).join(" · ")]
          : [Wrench, call.name, target];
      }
    }
  }
}

export function ExecCard({ item, op }: { item: ToolItem; op: ExecOp }) {
  const { cwd } = useContext(DetailState);
  const features = new Set(op.features);
  const important = (call: ExecCall) => call.kind === "patch"
    || call.kind === "command" && !["read", "search", "find", "list", "count", "inspect", "git", "git_log", "git_read", "git_diff", "other"].includes(call.shell.intent)
    || call.kind === "tool" && ["apply_patch", "update_plan", "image_gen__imagegen"].includes(call.name);
  const calls = op.calls.slice(0, 2);
  if (calls.length === 2 && !calls.some(important)) {
    const effect = op.calls.slice(2).find(important);
    if (effect) calls[1] = effect;
  }
  const targets = calls.map(call => execTarget(call, cwd));
  const title = targets.map(([, label, target]) => [label, target].filter(Boolean).join(" ")).join(" · ")
    || (features.has("invalid") ? "JavaScript 语法异常" : features.has("state") ? "处理脚本状态"
      : features.has("output") ? "处理脚本数据" : "JavaScript");
  const details = targets.map(([, label, target, full]) => [label, full ?? target].filter(Boolean).join(" ")).join("\n") || title;
  const badges = [
    features.has("parallel") && "并行",
    features.has("loop") && "循环",
    features.has("conditional") && "含条件",
    op.calls.some(call => call.kind === "tool" && call.dynamic) && "动态参数",
    !targets.length && `${op.lines} 行`,
    op.calls.length > targets.length && `+${op.calls.length - targets.length} 处引用`,
  ];
  return <OpCard item={item} className="exec" icon={targets.length === 1 ? targets[0][0] : targets.length ? ListTree : SquareCode} label="JavaScript 工具脚本"
    head={<><span className="op-title" title={details}>{title}</span><Stats items={badges} /></>}>
    <ToolSections<ExecOp> item={item} input={{ sourceLabel: "源代码", views: full => [
      full.calls.length > 0 && { label: "操作", body: <ExecCalls calls={full.calls} cwd={cwd} /> },
    ] }} output={{ views: result => {
      const decoded = scriptView(result);
      const diff = result && plainResultText(result) !== undefined && diffText(decoded?.copy);
      return [diff, decoded];
    } }} />
  </OpCard>;
}

function ExecCalls({ calls, cwd }: { calls: ExecCall[]; cwd: string | null }) {
  return <ol className="exec-calls">{calls.map((call, index) => {
    const [icon, label, target, full] = execTarget(call, cwd);
    const head = <><UiIcon icon={icon} size={14} /><span>{label}</span><code title={full ?? target}>{target}</code></>;
    if (call.kind === "command") return <li key={index}><details>
      <summary>{head}</summary>
      {call.cwd && <p className="exec-note" title={call.cwd}>{displayPath(call.cwd, cwd)}</p>}
      {commandSteps(call.shell.actions)?.body}
      {commandView(call.shell.command)?.body}
    </details></li>;
    if (call.kind === "patch") return <li key={index}><details>
      <summary>{head}</summary>
      <ul className="exec-changes">{call.changes.map((change, i) => {
        const view = changeView(change);
        const file = displayPath(change.path, cwd);
        const name = <code title={change.path}>{file}</code>;
        const action = change.kind === "edit" ? `+${change.added} −${change.removed}` : change.kind === "write" ? "新建" : "删除";
        return <li key={i}>{view ? <details><summary>{name}<span>{action}</span></summary>{view.body}</details>
          : <div>{name}<span>{action}</span></div>}</li>;
      })}</ul>
    </details></li>;
    if (call.kind === "tool" && (call.fields?.length || call.dynamic)) return <li key={index}><details>
      <summary>{head}{call.dynamic && <small>动态参数</small>}</summary>
      <ExecArguments call={call} />
    </details></li>;
    return <li key={index}><div>{head}</div></li>;
  })}</ol>;
}

function ExecArguments({ call }: { call: Extract<ExecCall, { kind: "tool" }> }) {
  const fields = call.fields ?? [];
  const input = Object.fromEntries(fields);
  const web = call.name === "web__run" ? webScriptSearch(input) : undefined;
  const docsSearch = call.name === "search_openai_docs" || call.name === "mcp__openaiDeveloperDocs__search_openai_docs";
  const docs = docsSearch && typeof input.query === "string" ? webSearchView([input.query], input) : undefined;
  const remaining = docs ? [] : web?.additional ?? fields;
  return <div className="exec-fields">
    {web?.body ?? docs?.body}
    {remaining.length > 0 && <ExpandableFrame label="已知参数">{() => <dl className="tool-arguments">
      {remaining.map(([key, value]) => <div key={key}><dt>{key}</dt><dd>
        <pre className="code">{value === "" ? '""' : payloadText(value)}</pre>
      </dd></div>)}
    </dl>}</ExpandableFrame>}
    {call.expression && textView("参数表达式", call.expression, null).body}
    {!call.expression && call.dynamic && <p className="exec-note">参数由脚本运行时确定</p>}
  </div>;
}

/** Codex 的 js 与 OpenCode 的 execute：标题优先用脚本自带的说明。 */
export function ScriptCard({ item, op }: { item: ToolItem; op: ScriptOp }) {
  return <OpCard item={item} className="command script" icon={SquareCode} label="脚本"
    head={<>
      {op.title ? <span className="op-title" title={op.title}>{op.title}</span> : <code className="op-name cmd" title={op.first}>{op.first}</code>}
      <Stats items={[`${op.lang ? `${op.lang} ` : ""}${op.lines} 行`]} />
    </>}>
    <ToolSections<ScriptOp> item={item}
      input={{ views: full => [textView(full.lang ?? "脚本", full.body ?? "")] }}
      output={{ views: result => [scriptView(result)] }} />
  </OpCard>;
}
