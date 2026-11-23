import { useContext, useMemo, type ReactNode } from "react";
import { FileMinus, FilePen, FilePlus, FileSearchCorner, FileText, FolderOpen, FolderSearch, ScanSearch, type LucideIcon } from "lucide-react";
import { browseSteps, compactPath, displayPath, itemStatus, splitPath, type ToolItem } from "../activity";
import { foldUnchanged, hunkRows, type DiffRow } from "../diff";
import type { BrowseOp, ChangeOp, ContentBlock, SearchHits, ShellFilter, ShellOp } from "../types";
import CopyButton from "./CopyButton";
import { DetailState } from "./DetailState";
import { Chip, CwdTag, FullOp, OpCard, ResultOutput, ToolBody, useRawView } from "./OpCard";
import { ExpandableFrame, Payload, resultText, ToolInput, ToolOutput, ToolResult, ToolSection } from "./ToolPayload";
import UiIcon from "./UiIcon";

type Step = BrowseOp & { program?: string; filters?: ShellFilter[] };

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
  return `L${op.start}–${op.end}`;
}

function searchMeta(op: Extract<BrowseOp, { kind: "search" }>) {
  if (op.matches === 0 || op.files === 0) return "无匹配";
  if (op.matches === undefined) return op.files === undefined ? undefined : `${op.files} 个文件`;
  return op.in_file && op.paths?.length === 1 ? `${op.matches} 处` : `${op.matches} 处 · ${op.files} 个文件`;
}

/** 目标名在前，范围或目录缩写在后，统计放在标签里。 */
export function browseTarget(op: BrowseOp, cwd: string | null) {
  const scope = (paths?: string[]) => paths?.length ? paths.map(path => displayPath(path, cwd)).join(", ") : undefined;
  switch (op.kind) {
    case "read": {
      const { dir, name } = splitPath(displayPath(op.path, cwd));
      return { name, dir, title: op.path, meta: readMeta(op) };
    }
    case "search":
      return { name: shownPattern(op.pattern), dir: scope(op.paths), title: op.pattern, meta: searchMeta(op), pattern: true };
    case "find":
      return { name: op.pattern ?? "全部文件", dir: scope(op.paths), title: op.pattern, meta: op.count === undefined ? undefined : `${op.count} 个`, pattern: !!op.pattern };
    case "list": {
      const path = op.paths?.[0] ? displayPath(op.paths[0], cwd) : ".";
      const { dir, name } = splitPath(path);
      const more = (op.paths?.length ?? 0) > 1 ? ` 等 ${op.paths!.length} 个` : "";
      return { name: (name || path) + more, dir, title: op.paths?.join("\n"), meta: op.count === undefined ? undefined : `${op.count} 项` };
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

/** 统计标签不收缩；管道筛选可被截断，截取行数这类短标签保持完整。 */
export function OpMeta({ meta, filters }: { meta?: ReactNode; filters?: ShellFilter[] }) {
  if (!meta && !filters?.length) return null;
  return <span className="op-meta">
    {meta && <Chip tone="sans">{meta}</Chip>}
    {filters?.map((filter, i) => <span key={i} className={`op-chip${filter.kind === "include" || filter.kind === "exclude" ? " shrink" : ""}`}
      title={filterLabel(filter)}>{filterLabel(filter)}</span>)}
  </span>;
}

/** 读取、搜索、查找、列目录；只做这些事的命令按步骤展示，同一张卡片。 */
export function BrowseCard({ item, shell }: { item: ToolItem; shell?: ShellOp }) {
  const { cwd } = useContext(DetailState);
  const [first, ...rest] = browseSteps(item.op) as Step[];
  const [icon, label] = browseIcon(first);
  const target = browseTarget(first, cwd);
  const programs = shell && [...new Set(shell.steps!.map(step => step.program))].join(" · ");
  return <OpCard item={item} className={first.kind} icon={icon} label={label} tone="dim" tag={programs}
    head={<>
      <code className={`op-name${target.pattern ? " pattern" : ""}${rest.length ? " multi" : ""}`} title={target.title}>{target.name}</code>
      {target.dir && <span className="op-dir" title={target.dir}>{compactPath(target.dir)}</span>}
      <OpMeta meta={target.meta} filters={first.filters} />
      {rest.slice(0, 2).map((step, i) => {
        const [stepIcon, stepLabel] = browseIcon(step);
        const { name, pattern } = browseTarget(step, cwd);
        return <span key={i} className="op-step" title={stepLabel}>
          <UiIcon icon={stepIcon} size={12} /><code className={pattern ? "pattern" : undefined}>{name}</code>
        </span>;
      })}
      {rest.length > 2 && <span className="op-more">+{rest.length - 2}</span>}
      <CwdTag dir={shell?.cwd} />
      {shell?.description && <span className="op-detail" title={shell.description}>{shell.description}</span>}
    </>}>
    <BrowseBody item={item} />
  </OpCard>;
}

/** 命令要显示完整命令、搜索要显示分组命中，这两种才读取调用正文；其余只读结果。 */
function BrowseBody({ item }: { item: ToolItem }) {
  const { raw, toggle } = useRawView();
  if (raw) return <ToolBody item={item} actions={toggle} />;
  if (item.op?.kind !== "shell" && item.op?.kind !== "search") return <BrowseView item={item} op={item.op as BrowseOp} toggle={toggle} />;
  return <FullOp<BrowseOp | ShellOp> item={item}>{(op, call) => <BrowseView item={item} op={op} call={call} toggle={toggle} />}</FullOp>;
}

function BrowseView({ item, op, call, toggle }: { item: ToolItem; op: BrowseOp | ShellOp; call?: ContentBlock; toggle: ReactNode }) {
  const { cwd } = useContext(DetailState);
  const steps = (browseSteps(op) ?? []) as Step[];
  const shell = op.kind === "shell";
  const command = shell && <ToolInput value={call?.input} command={op.command} actions={toggle} />;
  const actions = shell ? undefined : toggle;
  const [step] = steps;
  if (steps.length > 1) {
    return <>
      {command}
      <ToolSection kind="output" title={`${steps.length} 步`}>
        <ul className="op-steps">{steps.map((step, i) => {
          const [icon, label] = browseIcon(step);
          const target = browseTarget(step, cwd);
          const detail = [target.dir, target.meta, ...(step.filters ?? []).map(filterLabel)].filter(Boolean).join(" · ");
          return <li key={i} title={label}><UiIcon icon={icon} size={12} /><code className={target.pattern ? "pattern" : undefined}>{target.name}</code>
            {detail && <span>{detail}</span>}</li>;
        })}</ul>
      </ToolSection>
      <ResultOutput item={item} />
    </>;
  }
  switch (step?.kind) {
    case "search":
      return <>{command}{step.hits
        ? <ToolSection kind="output" title="结果" actions={actions}><HitList hits={step.hits} pattern={step.pattern} /></ToolSection>
        : <ResultOutput item={item} title="结果" actions={actions} />}</>;
    case "find":
      return <>{command}<PathResult item={item} actions={actions} /></>;
    case "read":
      return <>{command}<ResultOutput item={item} title="内容" start={step.start ?? null} actions={actions} /></>;
    default:
      return <>{command}<ResultOutput item={item} title="结果" actions={actions} /></>;
  }
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

/** 搜索命中按文件分组：行号、命中高亮，上下文行变淡，行号不连续处标出省略。 */
function HitList({ hits, pattern }: { hits: SearchHits[]; pattern: string }) {
  const { cwd } = useContext(DetailState);
  const regex = useMemo(() => literalHighlight(pattern), [pattern]);
  const lines = hits.reduce((sum, file) => sum + 1 + file.lines.length, 0);
  return <ExpandableFrame lines={lines} label="搜索结果">{() => <div className="search-hits">{hits.map(file => {
    const { dir, name } = splitPath(displayPath(file.path, cwd));
    const matches = file.lines.filter(hit => !hit.context).length;
    return <div key={file.path} className="search-file">
      <div className="search-file-name"><UiIcon icon={FileText} size={12} /><span>{dir && <span className="search-dir">{dir}/</span>}{name}</span>
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
function PathResult({ item, actions }: { item: ToolItem; actions?: ReactNode }) {
  const { cwd } = useContext(DetailState);
  const { result } = item.part;
  if (!result) return <ResultOutput item={item} title="结果" actions={actions} />;
  const failed = itemStatus(item) === "error";
  return <Payload block={result} kind={failed ? "error" : "output"}>{loaded => {
    const text = resultText(loaded);
    const paths = text.split("\n").map(line => line.trim()).filter(Boolean);
    if (failed || !paths.length) return <ToolResult result={loaded} failed={failed} title="结果" start={null} actions={actions} />;
    return <ToolOutput text={text} title="结果" actions={actions}>
      <ExpandableFrame lines={paths.length} label="结果">{() => <div className="path-list">{paths.map((path, i) => {
        const { dir, name } = splitPath(displayPath(path, cwd));
        return <div key={i}>{dir && <span className="search-dir">{dir}/</span>}{name}</div>;
      })}</div>}</ExpandableFrame>
    </ToolOutput>;
  }}</Payload>;
}

const changeIcons: Record<ChangeOp["kind"], [LucideIcon, string, "warning" | "success" | "danger"]> = {
  edit: [FilePen, "修改文件", "warning"], write: [FilePlus, "写入文件", "success"], delete: [FileMinus, "删除文件", "danger"],
};

/** 修改、写入和删除文件；改动默认展开。 */
export function ChangeCard({ item, op }: { item: ToolItem; op: ChangeOp }) {
  const { cwd } = useContext(DetailState);
  const { dir, name } = splitPath(displayPath(op.path, cwd));
  const [icon, label, tone] = changeIcons[op.kind];
  return <OpCard item={item} className={op.kind} icon={icon} label={label} tone={tone} open
    head={<>
      <code className="op-name" title={op.path}>{name}</code>
      {dir && <span className="op-dir" title={dir}>{compactPath(dir)}</span>}
      <span className="op-meta">
        {op.kind === "edit" && <><span className="op-added">+{op.added}</span><span className="op-removed">−{op.removed}</span></>}
        {op.kind === "write" && <>{op.created ? <Chip tone="created">新建</Chip> : <span>写入</span>}<span>{op.lines} 行</span></>}
        {op.kind === "delete" && <Chip tone="removed">删除</Chip>}
      </span>
    </>}>
    <ChangeBody item={item} />
  </OpCard>;
}

function ChangeBody({ item }: { item: ToolItem }) {
  const { onCopyError } = useContext(DetailState);
  const { raw, toggle } = useRawView();
  const failed = itemStatus(item) === "error";
  const path = (item.op as ChangeOp).path;
  const actions = <>
    <CopyButton className="content-action btn-ghost btn-sm" label="复制路径" value={path} onError={onCopyError}>路径</CopyButton>
    {toggle}
  </>;
  if (raw) return <ToolBody item={item} actions={actions} />;
  return <>
    <FullOp<ChangeOp> item={item}>{op => {
      if (op.kind === "write") return <ToolOutput text={op.content ?? ""} title="内容" actions={actions} />;
      if (op.kind === "delete") return <ToolSection kind="output" title="删除" actions={actions}><p className="tool-payload-message">文件已删除</p></ToolSection>;
      const rows = hunkRows(op.hunks ?? []);
      const folded = foldUnchanged(rows, 2);
      const hasFolded = folded.length !== rows.length || folded.some((row, index) => row.kind !== rows[index].kind);
      return <ToolSection kind="output" title="修改" actions={actions}>
        <ExpandableFrame lines={rows.length} overflow={hasFolded} label="修改">{expanded => <DiffRows rows={expanded ? rows : folded} />}</ExpandableFrame>
      </ToolSection>;
    }}</FullOp>
    {failed && <ResultOutput item={item} />}
  </>;
}

function DiffRows({ rows }: { rows: DiffRow[] }) {
  return <div className="tool-diff">{rows.map((row, index) => row.kind === "skip"
    ? <div key={index} className="tool-diff-skip">{row.count === null ? "⋯" : `⋯ ${row.count} 行未改动`}</div>
    : <div key={index} className={`tool-diff-row ${row.kind}`}>
      <span className="tool-diff-number">{row.after ?? row.before ?? ""}</span>
      <span className="tool-diff-sign">{row.kind === "add" ? "+" : row.kind === "remove" ? "−" : ""}</span>
      <span className="tool-diff-text">{row.text || " "}</span>
    </div>)}</div>;
}
