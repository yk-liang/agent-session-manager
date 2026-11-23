import { useContext } from "react";
import {
  Activity, AppWindow, CircleStop, Copy, Database, ExternalLink, FilePen, FilePlus, FileSearchCorner, FileText, FlaskConical,
  FolderOpen, FolderPlus, GitBranch, GitCommitHorizontal, GitCompare, GitPullRequest, Globe, Hammer, Hash, History, Hourglass,
  Move, Package, ScanSearch, Server, SquareCode, SquareTerminal, Trash, type LucideIcon,
} from "lucide-react";
import { compactPath, displayPath, splitPath, titlePaths, type ToolItem } from "../activity";
import type { ScriptInfo, ShellIntent, ShellOp, ToolOp } from "../types";
import { DetailState } from "./DetailState";
import { OpMeta } from "./FileCards";
import { Chip, CwdTag, FullOp, OpCard, ResultOutput, type Tone } from "./OpCard";
import { ShellText, ToolInput, ToolOutput } from "./ToolPayload";

const intents: Record<ShellIntent, [LucideIcon, string, Tone?]> = {
  read: [FileText, "读取文件", "dim"], search: [FileSearchCorner, "搜索", "dim"], find: [ScanSearch, "查找文件", "dim"],
  list: [FolderOpen, "列出目录", "dim"], write: [FilePlus, "写入文件", "success"], modify: [FilePen, "修改文件", "warning"],
  script: [SquareCode, "脚本"], run: [SquareTerminal, "运行程序"],
  git_diff: [GitCompare, "Git 查看改动"], git_log: [History, "Git 历史"], git_commit: [GitCommitHorizontal, "Git 提交"],
  git: [GitBranch, "Git"], github: [GitPullRequest, "GitHub CLI"],
  test: [FlaskConical, "测试"], build: [Hammer, "构建与检查"], install: [Package, "安装依赖"],
  remote: [Server, "远程执行"], http: [Globe, "HTTP 请求"], browser: [AppWindow, "浏览器自动化"], database: [Database, "数据库"],
  mkdir: [FolderPlus, "创建目录"], remove: [Trash, "删除", "danger"], copy: [Copy, "复制"], move: [Move, "移动"],
  process: [Activity, "查看进程"], kill: [CircleStop, "结束进程"], wait: [Hourglass, "等待"], open: [ExternalLink, "打开"],
  count: [Hash, "统计", "dim"], other: [SquareTerminal, "命令"],
};

/** 不只是读、搜、找、列的命令：标题是第一步命令本身，去掉了目录切换、环境变量和无关重定向。 */
export function CommandCard({ item, op }: { item: ToolItem; op: ShellOp }) {
  const { cwd } = useContext(DetailState);
  const [icon, label, tone] = intents[op.intent];
  const file = op.target && (op.intent === "write" || op.intent === "modify") ? splitPath(displayPath(op.target, cwd)) : undefined;
  const fileAction = op.intent === "modify" ? "原地修改" : op.append ? "追加" : "写入";
  const more = op.more ? `+${op.more} 步` : undefined;
  const detail = op.description ?? (op.script && !file ? op.script.first : undefined);
  return <OpCard item={item} className={`command ${op.intent}`} icon={icon} label={label} tone={tone}
    head={<>
      {file ? <>
        <code className="op-name" title={op.target}>{file.name}</code>
        {file.dir && <span className="op-dir">{compactPath(file.dir)}</span>}
        <span className="op-meta"><Chip tone="sans">{op.script ? `${fileAction} ${op.script.lines} 行` : fileAction}</Chip>{more && <Chip tone="sans">{more}</Chip>}</span>
      </> : <>
        <code className="op-name cmd" title={op.head}><ShellText command={titlePaths(op.head ?? "", cwd)} /></code>
        {op.looped ? <span className="op-meta"><Chip tone="sans">循环</Chip></span> : <>
          {op.script && <span className="op-meta"><Chip tone="sans">{op.script.lang ? `${op.script.lang} ` : ""}{op.script.lines} 行</Chip></span>}
          <OpMeta meta={more} filters={op.filters} />
        </>}
      </>}
      <CwdTag dir={op.cwd} />
      {detail && <span className={`op-detail${op.description ? "" : " code-detail"}`} title={detail}>{detail}</span>}
    </>}>
    <FullOp<ShellOp> item={item}>{(full, call) => <>
      {full.script?.body !== undefined && <ScriptSection script={full.script} title={file ? "内容" : undefined} />}
      <ToolInput value={call.input} command={full.command} />
      <ResultOutput item={item} />
    </>}</FullOp>
  </OpCard>;
}

function ScriptSection({ script, title }: { script: ScriptInfo; title?: string }) {
  return <ToolOutput text={script.body ?? ""} title={title ?? (script.lang ? `${script.lang} 脚本` : "标准输入")} />;
}

/** 脚本工具（Codex 的 js、exec 包装脚本，OpenCode 的 execute）：标题优先用脚本自带的说明。 */
export function ScriptCard({ item, op }: { item: ToolItem; op: Extract<ToolOp, { kind: "script" }> }) {
  return <OpCard item={item} className="command script" icon={SquareCode} label="脚本"
    head={<>
      {op.title ? <span className="op-title" title={op.title}>{op.title}</span> : <code className="op-name cmd" title={op.first}>{op.first}</code>}
      <span className="op-meta"><Chip tone="sans">{op.lang ? `${op.lang} ` : ""}{op.lines} 行</Chip></span>
    </>}>
    <FullOp<Extract<ToolOp, { kind: "script" }>> item={item}>{full => <ScriptSection script={full} />}</FullOp>
    <ResultOutput item={item} />
  </OpCard>;
}
