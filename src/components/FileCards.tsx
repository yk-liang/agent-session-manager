import { useContext, useMemo, type ReactNode } from "react";
import { FileMinus, FilePen, FilePlus, FileSearchCorner, FileText, FolderOpen, FolderSearch, ScanSearch, type LucideIcon } from "lucide-react";
import { browseSteps, commandOutcome, itemStatus, type ToolItem } from "../activity";
import type { BrowseOp, ChangeOp, SearchHits, ShellFilter, ShellOp } from "../types";
import { compactPath, displayPath, shortPath, splitPath } from "../utils";
import CopyButton from "./CopyButton";
import { DetailState } from "./DetailState";
import DiffView from "./DiffView";
import { Chip, OpCard, PathName, Stats, StepList, type Badges, type Head, type StepRow } from "./OpCard";
import { diffResult, documentResult, plainResultText } from "./ResultViews";
import { commandView, ExpandableFrame, textView } from "./ToolContent";
import { ToolSections } from "./ToolSections";
import UiIcon from "./UiIcon";

type Step = BrowseOp & { filters?: ShellFilter[] };

function browseIcon(op: BrowseOp): [LucideIcon, string] {
  switch (op.kind) {
    case "read": return [FileText, "读取文件"];
    case "search": return op.in_file ? [FileSearchCorner, "在文件中搜索"] : [FolderSearch, "在文件夹中搜索"];
    case "find": return [ScanSearch, "按名称查找文件"];
    case "list": return [FolderOpen, "列出目录"];
  }
}

/** `grep` 的基本正则里 `\|` 才是“或”，显示时还原成 `|`。 */
function shownPattern(pattern: string) {
  return pattern.replace(/\\\|/g, "|");
}

function readMeta(op: Extract<BrowseOp, { kind: "read" }>) {
  if (op.tail !== undefined) return `末 ${op.tail} 行`;
  if (op.start === undefined || op.end === undefined) return;
  if (op.start === 1 && op.total !== undefined && op.end >= op.total) return `${op.total} 行`;
  return `L${op.start}–${op.end}${op.total === undefined ? "" : ` / ${op.total}`}`;
}

function searchMeta(op: Extract<BrowseOp, { kind: "search" }>) {
  if (op.matches === 0 || op.files === 0) return "无匹配";
  if (op.matches === undefined) return op.files === undefined ? undefined : `${op.files} 个文件`;
  return op.in_file && op.paths?.length === 1 ? `${op.matches} 处` : `${op.matches} 处 · ${op.files} 个文件`;
}

/** 目标名在前，范围或目录缩写在后，统计放在标签里。 */
export function browseTarget(op: BrowseOp, cwd: string | null) {
  // 范围就是会话目录时不必再写一个「.」。
  const scope = (paths?: string[]) => paths?.map(path => displayPath(path, cwd)).filter(path => path !== ".").join(", ") || undefined;
  switch (op.kind) {
    case "read": {
      const { dir, name } = splitPath(displayPath(op.path, cwd));
      return { name, dir, title: op.path, meta: readMeta(op) };
    }
    case "search":
      return { name: shownPattern(op.pattern), dir: scope(op.paths), dirTitle: op.paths?.join("\n"), title: op.pattern, meta: searchMeta(op), pattern: true };
    case "find":
      return { name: op.pattern ?? "全部文件", dir: scope(op.paths), dirTitle: op.paths?.join("\n"), title: op.pattern, meta: op.count === undefined ? undefined : `${op.count} 个`, pattern: !!op.pattern };
    case "list": {
      const path = op.paths?.[0] ? displayPath(op.paths[0], cwd) : ".";
      const { dir, name } = splitPath(path === "." && cwd ? shortPath(cwd) : path);
      const more = (op.paths?.length ?? 0) > 1 ? ` 等 ${op.paths!.length} 个` : "";
      const title = path === "." && cwd && (!op.paths?.length || op.paths.length === 1 && op.paths[0] === ".") ? cwd : op.paths?.join("\n");
      return { name: (name || path) + more, dir, title, meta: op.count === undefined ? undefined : `${op.count} 项` };
    }
  }
}

function filterLabel(filter: ShellFilter) {
  switch (filter.kind) {
    case "head": return `前 ${filter.value} 行`;
    case "tail": return `后 ${filter.value} 行`;
    case "include": return `筛选 ${shownPattern(filter.value)}`;
    case "exclude": return `排除 ${shownPattern(filter.value)}`;
    case "count": return "计数";
  }
}

/** 浏览卡片的标题：第一步的目标，统计放在标签里；`badges` 是调用方追加的标签。 */
export function browseHead(steps: BrowseOp[], cwd: string | null, badges: Badges = []): Head {
  const [first] = steps;
  const [icon, label] = browseIcon(first);
  const target = browseTarget(first, cwd);
  return { icon, label, tone: "dim", head: <>
    <code className={`op-name${target.pattern ? " pattern" : ""}`} title={target.title}>{target.name}</code>
    {target.dir && <span className="op-dir" title={target.dirTitle ?? target.title}>{compactPath(target.dir)}</span>}
    <Stats items={[target.meta, first.kind === "search" && first.truncated && "已截断", ...badges]} />
  </> };
}

export function browseRow(step: Step, cwd: string | null): StepRow {
  const [icon, label] = browseIcon(step);
  const target = browseTarget(step, cwd);
  return { icon, label, text: target.name, pattern: target.pattern, title: target.dirTitle ? `${target.title ?? target.name}\n${target.dirTitle}` : target.title,
    detail: [target.dir, target.meta, ...(step.filters ?? []).map(filterLabel)].filter(Boolean).join(" · ") };
}

/** 读取、搜索、查找、列目录；只做这些事的命令按步骤展示，同一张卡片。 */
export function BrowseCard({ item }: { item: ToolItem }) {
  const { cwd } = useContext(DetailState);
  const steps = browseSteps(item.op) as Step[];
  const { icon, label, tone, head } = browseHead(steps, cwd, [commandOutcome(item) === "no_match" && "无匹配", steps.length > 1 && `+${steps.length - 1} 步`]);
  return <OpCard item={item} className={steps[0].kind} icon={icon} label={label} tone={tone} head={head}>
    <BrowseBody item={item} steps={steps} />
  </OpCard>;
}

function BrowseBody({ item, steps }: { item: ToolItem; steps: Step[] }) {
  const { cwd } = useContext(DetailState);
  const shell = item.op?.kind === "shell" ? item.op : undefined;
  const [step] = steps;
  const targets = step.kind === "read" ? [step.path] : step.paths ?? [];
  const pattern = step.kind === "search" || step.kind === "find" ? step.pattern : undefined;
  const summary = [pattern, ...targets]
    .filter(Boolean).join("\n");
  const single = steps.length === 1 ? step : undefined;
  return <ToolSections<BrowseOp | ShellOp> item={item} input={{
    note: shell?.description ?? item.part.content.description,
    preview: !shell && summary ? textView(step.kind === "read" ? "路径" : "目标", summary, null) : undefined,
    loadOnExpand: single?.kind === "search",
    views: full => [
      (steps.length > 1 || !!step.filters?.length) && { label: "步骤", body: <StepList rows={steps.map(step => browseRow(step, cwd))} /> },
      full.kind === "shell" && commandView(full.command),
    ],
  }} output={{
    title: !single ? "合并输出" : single.kind === "read" ? "内容" : "结果",
    start: single?.kind === "read" ? single.start : undefined,
    views: (result, full) => {
      if (single?.kind === "read") return [diffResult(result), /\.(?:md|markdown|mdx|mkd)$/i.test(single.path) && documentResult(result)];
      if (single?.kind === "search") {
        const [search] = browseSteps(full) ?? [];
        return [search?.kind === "search" && !!search.hits?.length && { label: "命中", body: <HitList hits={search.hits} pattern={search.pattern} /> }];
      }
      if (single?.kind === "find" && result && single.count !== 0 && !result.is_error && !["error", "denied", "cancelled"].includes(itemStatus(item) ?? "")) {
        const text = plainResultText(result);
        const paths = text ? text.split("\n").map(line => line.trim())
          .filter(line => line && !/^No .+ found$|^Found \d+ /i.test(line)) : [];
        return [paths.length > 0 && { label: "路径", body: <PathList paths={paths} />, meta: `${paths.length} 个`, copy: paths.join("\n") }];
      }
      return [];
    },
  }} />;
}

function literalHighlight(pattern: string) {
  if (!pattern || pattern.length > 1024) return;
  const terms = shownPattern(pattern).split("|");
  if (terms.some(term => !term || /[\\()[\]{}^$*+?]/.test(term))) return;
  return new RegExp(terms.map(term => term.replace(/\./g, "\\.")).join("|"), "gi");
}

function Highlighted({ text, regex }: { text: string; regex: RegExp }) {
  const parts: ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(regex)) {
    if (!match[0]) continue;
    parts.push(text.slice(last, match.index), <mark key={match.index}>{match[0]}</mark>);
    last = match.index + match[0].length;
  }
  parts.push(text.slice(last));
  return <>{parts}</>;
}

/** 搜索命中按文件分组：行号、命中高亮，上下文行变淡，行号不连续处以虚线分隔。 */
function HitList({ hits, pattern }: { hits: SearchHits[]; pattern: string }) {
  const { cwd } = useContext(DetailState);
  const regex = useMemo(() => literalHighlight(pattern), [pattern]);
  const lines = hits.reduce((sum, file) => sum + 1 + file.lines.length, 0);
  return <ExpandableFrame lines={lines} label="搜索结果">{() => <div className="search-hits">{hits.map(file => {
    const { dir, name } = splitPath(displayPath(file.path, cwd));
    const matches = file.lines.filter(hit => !hit.context).length;
    return <div key={file.path} className="search-file">
      <div className="search-file-name"><UiIcon icon={FileText} size={12} /><span title={file.path}>{dir && <span className="search-dir">{dir}/</span>}{name}</span>
        {matches > 0 && <span className="search-count">{matches}</span>}</div>
      {file.lines.map((hit, i) => {
        const previous = file.lines[i - 1]?.line;
        const gap = hit.line !== undefined && previous !== undefined && hit.line > previous + 1;
        return <div key={i} className={`search-hit${hit.context ? " context" : ""}${gap ? " gap" : ""}`}>
          <span className="search-line">{hit.line ?? ""}</span>
          <span className="search-text">{hit.context || !regex ? hit.text : <Highlighted text={hit.text} regex={regex} />}</span>
        </div>;
      })}
    </div>;
  })}</div>}</ExpandableFrame>;
}

/** 查找结果按路径列出：目录变淡，文件名突出。 */
function PathList({ paths }: { paths: string[] }) {
  const { cwd } = useContext(DetailState);
  return <ExpandableFrame lines={paths.length} label="路径">{() => <div className="path-list">{paths.map((path, i) => {
    const { dir, name } = splitPath(displayPath(path, cwd));
    return <div key={i} title={path}>{dir && <span className="search-dir">{dir}/</span>}{name}</div>;
  })}</div>}</ExpandableFrame>;
}

const changeIcons: Record<ChangeOp["kind"], [LucideIcon, string, "warning" | "success" | "danger"]> = {
  edit: [FilePen, "修改文件", "warning"], write: [FilePlus, "写入文件", "success"], delete: [FileMinus, "删除文件", "danger"],
};

export function changeHead(op: ChangeOp, cwd: string | null, badges: Badges = []): Head {
  const [icon, label, tone] = changeIcons[op.kind];
  return { icon, label, tone, head: <>
    <PathName path={op.path} cwd={cwd} />
    <span className="op-meta">
      {op.kind === "edit" && <><span className="op-added">+{op.added}</span><span className="op-removed">−{op.removed}</span></>}
      {op.kind === "write" && <>{op.created ? <Chip tone="created">新建</Chip> : <span>写入</span>}<span>{op.lines} 行</span></>}
      {op.kind === "delete" && <Chip tone="removed">删除</Chip>}
    </span>
    <Stats items={badges} />
  </> };
}

export function changeRow(op: ChangeOp, cwd: string | null): StepRow {
  const [icon, label] = changeIcons[op.kind];
  return { icon, label, text: displayPath(op.path, cwd), title: op.path,
    detail: op.kind === "edit" ? `+${op.added} −${op.removed}` : op.kind === "write" ? `${op.created ? "新建 " : ""}${op.lines} 行` : undefined };
}

export function ChangeCard({ item, op }: { item: ToolItem; op: ChangeOp }) {
  const { cwd } = useContext(DetailState);
  const { icon, label, tone, head } = changeHead(op, cwd);
  return <OpCard item={item} className={op.kind} icon={icon} label={label} tone={tone} head={head}>
    <ChangeBody item={item} path={op.path} />
  </OpCard>;
}

function ChangeBody({ item, path }: { item: ToolItem; path: string }) {
  const { onCopyError } = useContext(DetailState);
  const status = itemStatus(item);
  return <ToolSections<ChangeOp> item={item} input={{
    actions: <CopyButton className="content-action btn-ghost btn-sm" label="复制路径" value={path} onError={onCopyError}>路径</CopyButton>,
    views: op => [changeView(op)],
  }} output={status === "error" || status === "denied" || item.index === 0 && !!item.part.content.script_result ? {} : false} />;
}

export function changeView(op: ChangeOp) {
  if (op.kind === "edit") return { label: "改动", body: <DiffView files={[{ hunks: op.hunks ?? [] }]} /> };
  if (op.kind === "write") return textView("内容", op.content ?? "");
}
