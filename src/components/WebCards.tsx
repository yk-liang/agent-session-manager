import { Globe } from "lucide-react";
import type { ToolItem } from "../activity";
import type { ToolOp } from "../types";
import { Chip, OpCard } from "./OpCard";
import { webResult } from "./ResultViews";
import { ExpandableFrame, type View } from "./ToolContent";
import { ToolSections } from "./ToolSections";

type SearchOp = Extract<ToolOp, { kind: "web_search" }>;
type FetchOp = Extract<ToolOp, { kind: "web_fetch" }>;
type Field = [label: string, value: unknown];

function values(value: unknown): string[] {
  if (Array.isArray(value)) return value.filter((entry): entry is string => typeof entry === "string" && !!entry.trim());
  if (typeof value === "string" && value.trim() || typeof value === "number") return [String(value)];
  return [];
}

function rows(fields: Field[]) {
  return fields.map(([label, value]) => [label, values(value)] as const).filter(([, values]) => values.length);
}

function WebFields({ entries }: { entries: ReturnType<typeof rows> }) {
  return <dl className="tool-arguments web-arguments">
    {entries.map(([label, values]) => <div key={label}>
      <dt>{label}</dt>
      <dd>{values.length === 1 ? values[0] : <ul>{values.map((value, i) => <li key={i}>{value}</li>)}</ul>}</dd>
    </div>)}
  </dl>;
}

function fieldsView(label: string, fields: Field[]): View | undefined {
  const entries = rows(fields);
  if (!entries.length) return;
  return {
    label,
    copy: entries.map(([, values]) => values.join("\n")).join("\n"),
    body: <ExpandableFrame label={label}>{() => <WebFields entries={entries} />}</ExpandableFrame>,
  };
}

export function webSearchView(queries: string[], input?: unknown, domain?: string): View | undefined {
  const options = input && typeof input === "object" ? input as Record<string, unknown> : {};
  return fieldsView("查询", [
    ["搜索词", queries], ["限定域名", options.domain ?? domain], ["允许的域名", options.allowed_domains ?? options.domains],
    ["排除的域名", options.blocked_domains], ["搜索模式", options.mode], ["结果上限", options.limit ?? options.numResults],
    ["最近天数", options.recency], ["返回篇幅", options.response_length],
  ]);
}

const webQueries = ["search_query", "image_query", "literature_search_query"] as const;
export function webScriptSearch(fields: Record<string, unknown>) {
  const groups = webQueries.flatMap(key => {
    const entries = fields[key];
    if (!Array.isArray(entries)) return [];
    return entries.flatMap(entry => {
      if (!entry || typeof entry !== "object") return [];
      const fields = rows([["搜索词", entry.query], ["允许的域名", entry.domains], ["最近天数", entry.recency]]);
      return fields.length ? [fields] : [];
    });
  });
  if (!groups.length) return;
  const options = rows([["返回篇幅", fields.response_length]]);
  const additional = Object.entries(fields).filter(([key]) => key !== "response_length" && !webQueries.some(name => name === key));
  return { body: <ExpandableFrame label="查询">{() => <div className="web-search-batch">
    {groups.map((entries, i) => <WebFields key={i} entries={entries} />)}
    {options.length > 0 && <WebFields entries={options} />}
  </div>}</ExpandableFrame>, additional };
}

export function WebSearchCard({ item, op }: { item: ToolItem; op: SearchOp }) {
  const [first] = op.queries;
  return <OpCard item={item} className="web_search" icon={Globe} label="网页搜索" head={<>
    <span className="op-title" title={op.queries.join("\n") || undefined}>{first ?? "网页搜索"}</span>
    <span className="op-meta">
      {op.queries.length > 1 && <Chip tone="sans">{op.queries.length} 条查询</Chip>}
      {op.domain && <Chip>{op.domain}</Chip>}
      {op.results !== undefined && <Chip tone="sans">{op.results} 条结果</Chip>}
    </span>
  </>}>
    <ToolSections<SearchOp> item={item} input={{ views: (op, call) => [webSearchView(op.queries, call.input, op.domain)] }}
      output={{ title: "结果", views: result => [webResult(result)], preferScript: true }} />
  </OpCard>;
}

export function WebFetchCard({ item, op }: { item: ToolItem; op: FetchOp }) {
  const find = item.part.content.name === "find_in_page";
  const label = find ? "在网页中查找" : "读取网页";
  const target = (find && op.prompt) || op.url || label;
  return <OpCard item={item} className="web_fetch" icon={Globe} label={label}
    head={find ? <span className="op-title" title={target}>{target}</span>
      : <code className="op-name" title={target}>{target.replace(/^https?:\/\//, "")}</code>}>
    <ToolSections<FetchOp> item={item} input={{ views: (op, call) => [
      fieldsView("目标", [["网址", op.url], [find ? "查找词" : "提问", op.prompt],
        ["格式", (call.input as { format?: unknown } | undefined)?.format]]),
    ] }} output={{ title: find ? "结果" : "内容", views: result => [webResult(result, find ? "find" : "fetch")], preferScript: true }} />
  </OpCard>;
}
