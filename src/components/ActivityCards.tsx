import { useContext } from "react";
import { Brain, FileOutput, ListCollapse } from "lucide-react";
import { activityItems, browseSteps, itemStatus, type ToolItem } from "../activity";
import type { SessionBlock } from "../sessionContent";
import { CommandCard, ScriptCard } from "./CommandCards";
import Collapsible from "./Collapsible";
import { DetailState } from "./DetailState";
import { browseTarget, BrowseCard, ChangeCard } from "./FileCards";
import { LongText } from "./MessageParts";
import { Status, TypeIcon } from "./OpCard";
import { GenericToolCard, SemanticToolCard, SkillCard } from "./ToolCards";
import { Payload, ToolResult } from "./ToolPayload";

export function ActivityList({ parts }: { parts: SessionBlock[] }) {
  return <>{activityItems(parts).map(entry => {
    switch (entry.type) {
      case "thinking": return <ThinkingCard key={entry.key} part={entry.part} />;
      case "result": return <OrphanResult key={entry.key} part={entry.part} />;
      case "browse": return <BrowseGroup key={entry.key} id={entry.key} items={entry.items} />;
      default: return <ToolCard key={entry.item.key} item={entry.item} />;
    }
  })}</>;
}

function ThinkingCard({ part }: { part: SessionBlock }) {
  const { markdown } = useContext(DetailState);
  const text = part.content.thinking ?? "";
  return <Collapsible id={part.key} className="thinking" head={<>
    <TypeIcon icon={Brain} label="思考过程" />
    <span className="collapsible-preview closed-only">{text.trim().replace(/\s+/g, " ").slice(0, 240)}</span>
  </>}>
    <LongText text={text} code markdown={markdown} />
  </Collapsible>;
}

/** 找不到调用的结果单独成卡。 */
function OrphanResult({ part }: { part: SessionBlock }) {
  const failed = !!part.content.is_error;
  return <Collapsible id={part.key} className={`tool${failed ? " error" : ""}`} head={<>
    <TypeIcon icon={FileOutput} label="工具输出" />
    <span className="collapsible-status"><Status status={failed ? "error" : undefined} /></span>
  </>}>
    <Payload block={part.content} kind={failed ? "error" : "output"}>
      {loaded => <ToolResult result={loaded} failed={failed} />}
    </Payload>
  </Collapsible>;
}

function ToolCard({ item }: { item: ToolItem }) {
  const { op } = item;
  if (!op) return <GenericToolCard item={item} />;
  switch (op.kind) {
    case "read": case "search": case "find": case "list": return <BrowseCard item={item} />;
    case "edit": case "write": case "delete": return <ChangeCard item={item} op={op} />;
    case "shell": return op.steps?.length ? <BrowseCard item={item} shell={op} /> : <CommandCard item={item} op={op} />;
    case "script": return <ScriptCard item={item} op={op} />;
    case "skill": return <SkillCard item={item} op={op} />;
    default: return <SemanticToolCard item={item} op={op} />;
  }
}

/** 相邻的读取、搜索、查找、列目录合成一张：标题统计各类次数，展开后逐个查看。 */
function BrowseGroup({ id, items }: { id: string; items: ToolItem[] }) {
  const { cwd } = useContext(DetailState);
  const steps = items.flatMap(item => browseSteps(item.op) ?? []);
  const count = (kind: string) => steps.filter(step => step.kind === kind).length;
  const files = new Set(steps.flatMap(step => step.kind === "read" ? [step.path] : [])).size;
  const title = [files && `读取 ${files} 个文件`, count("search") && `搜索 ${count("search")} 次`,
    count("find") && `查找 ${count("find")} 次`, count("list") && `列出 ${count("list")} 次`].filter(Boolean).join("，");
  const names = [...new Set(steps.map(step => browseTarget(step, cwd).name))];
  const failures = items.filter(item => itemStatus(item) === "error").length;
  return <Collapsible id={id} className={`tool browse${failures ? " error" : ""}`} head={<>
    <TypeIcon icon={ListCollapse} label={`合并了 ${items.length} 次调用`} tone="dim" />
    <span className="browse-title">{title}</span>
    <span className="browse-names">
      {names.slice(0, 4).map(name => <code key={name}>{name}</code>)}
      {names.length > 4 && <span>+{names.length - 4}</span>}
    </span>
    <span className="collapsible-status">
      {failures > 0 && <span className="tool-status error">{failures} 次失败</span>}
      <span className="tool-tag">{items.length} 次调用</span>
    </span>
  </>}>
    {items.map(item => <ToolCard key={item.key} item={item} />)}
  </Collapsible>;
}
