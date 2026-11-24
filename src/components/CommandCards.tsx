import { useContext } from "react";
import {
  Activity, AppWindow, Archive, Bot, Braces, CircleStop, Copy, Database, Download, ExternalLink, FilePen, FilePlus, FileSearchCorner, FileText, KeyRound, Link2, ShieldCheck,
  FlaskConical, FolderOpen, FolderPlus, GitBranch, GitCommitHorizontal, GitCompare, GitPullRequest, Globe, Hammer, Hash, History,
  Hourglass, Info, Move, Package, ScanSearch, Server, SquareCode, SquareTerminal, Trash, type LucideIcon,
} from "lucide-react";
import { commandOutcome, type ToolItem } from "../activity";
import type { ShellIntent, ShellOp, ToolOp } from "../types";
import { titlePaths } from "../utils";
import { DetailState } from "./DetailState";
import { OpCard, PathName, Stats, StepList, type Badges, type Head, type StepRow, type Tone } from "./OpCard";
import { diffResult, scriptView } from "./ResultViews";
import { commandView, ShellText, textView } from "./ToolContent";
import { ToolSections } from "./ToolSections";

export const intents: Record<ShellIntent, [LucideIcon, string, Tone?]> = {
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

export function actionRow({ command, intent, target }: NonNullable<ShellOp["actions"]>[number]): StepRow {
  const [icon, label] = intents[intent];
  return { icon, label, text: <ShellText command={command} />, title: target ?? command };
}

/** 命令卡片的标题：写入类给文件，脚本给说明（Python 没有说明时只留行数），其余给命令开头。 */
export function commandHead(op: Omit<ShellOp, "kind">, cwd: string | null, description: string | undefined, badges: Badges = []): Head {
  const [icon, label, tone] = intents[op.intent];
  const file = op.target && ["write", "modify", "download"].includes(op.intent) ? op.target : undefined;
  const fileAction = op.intent === "modify" ? "原地修改" : op.intent === "download" ? "保存响应" : op.append ? "追加" : "写入";
  const inline = op.intent === "script" ? op.script : undefined;
  const python = inline?.lang === "Python";
  const title = inline && (description || (!python && inline.first));
  const related = [...op.related?.slice(0, 2).map(intent => intents[intent][1]) ?? [], op.looped && "循环"];
  return { icon, label, tone, head: file ? <>
    <PathName path={file} cwd={cwd} />
    <Stats items={[op.script ? `${fileAction} ${op.script.lines} 行` : fileAction, ...badges, ...related]} />
  </> : <>
    {title ? <span className="op-title" title={title}>{title}</span>
      : !python && op.head && <code className="op-name cmd" title={op.head}><ShellText command={titlePaths(op.head, cwd)} /></code>}
    <Stats items={[op.script && `${op.script.lang ? `${op.script.lang} ` : ""}${op.script.lines} 行`, ...badges, ...related]} />
  </> };
}

const diffIntents = new Set<ShellIntent>(["diff", "git_diff", "git_log"]);

function showsDiff(op: Omit<ShellOp, "kind">) {
  return diffIntents.has(op.intent) || !!op.actions?.some(action => diffIntents.has(action.intent));
}

export function CommandCard({ item, op }: { item: ToolItem; op: ShellOp }) {
  const { cwd } = useContext(DetailState);
  const description = op.description?.trim() || item.part.content.description?.trim();
  const outcome = commandOutcome(item);
  const { icon, label, tone, head } = commandHead(op, cwd, description,
    [outcome === "differences" && "有差异", outcome === "no_match" && "无匹配", !!op.more && `+${op.more} 步`]);
  const file = op.target && ["write", "modify", "download"].includes(op.intent);
  return <OpCard item={item} className={`command ${op.intent}`} icon={icon} label={label} tone={tone} head={head}>
    <ToolSections<ShellOp> item={item} input={{ note: [description, op.conditional && "含条件步骤"].filter(Boolean).join(" · "), views: full => {
      const script = full.script?.body !== undefined && textView(file ? "内容" : full.script.lang ?? "标准输入", full.script.body);
      const steps = !!full.actions?.length && { label: "步骤", body: <StepList rows={full.actions.map(actionRow)} /> };
      return op.intent === "script" || file && !full.conditional
        ? [script, steps, commandView(full.command)]
        : [steps, script, commandView(full.command)];
    } }} output={{ terminal: true, views: (result, full) => [showsDiff(full) && diffResult(result)] }} />
  </OpCard>;
}

export function scriptHead(op: ScriptOp, badges: Badges = []): Head {
  return { icon: SquareCode, label: "脚本", head: <>
    {op.title ? <span className="op-title" title={op.title}>{op.title}</span> : <code className="op-name cmd" title={op.first}>{op.first}</code>}
    <Stats items={[`${op.lang ? `${op.lang} ` : ""}${op.lines} 行`, ...badges]} />
  </> };
}

/** Codex 的 js 与 OpenCode 的 execute：标题优先用脚本自带的说明。 */
export function ScriptCard({ item, op }: { item: ToolItem; op: ScriptOp }) {
  const { icon, label, head } = scriptHead(op);
  return <OpCard item={item} className="command script" icon={icon} label={label} head={head}>
    <ToolSections<ScriptOp> item={item}
      input={{ views: full => [textView(full.lang ?? "脚本", full.body ?? "")] }}
      output={{ terminal: true, views: result => [scriptView(result, "结果", true)] }} />
  </OpCard>;
}
