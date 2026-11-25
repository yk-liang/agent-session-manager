import { Globe } from "lucide-react";
import type { ToolItem } from "../activity";
import type { ToolOp } from "../types";
import { Chip, OpCard, Stats, type Badges, type Head } from "./OpCard";
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

function searchFields(queries: string[], input?: unknown, domain?: string): Field[] {
  const options = input && typeof input === "object" ? input as Record<string, unknown> : {};
  return [
    ["搜索词", queries], ["限定域名", options.domain ?? domain], ["允许的域名", options.allowed_domains ?? options.domains],
    ["排除的域名", options.blocked_domains], ["搜索模式", options.mode], ["结果上限", options.limit ?? options.numResults],
    ["最近天数", options.recency], ["返回篇幅", options.response_length],
  ];
}

function webSearchView(queries: string[], input?: unknown, domain?: string): View | undefined {
  return fieldsView("查询", searchFields(queries, input, domain));
}

const webQueries = ["search_query", "image_query", "literature_search_query"] as const;

function searchGroups(tool: string, queries: string[], input?: unknown) {
  if (tool !== "web__run") {
    const fields = rows(searchFields(queries, input));
    return fields.length ? [fields] : [];
  }
  const options = input && typeof input === "object" ? input as Record<string, unknown> : {};
  const groups = webQueries.flatMap(key => {
    const entries = options[key];
    if (!Array.isArray(entries)) return [];
    return entries.flatMap(entry => {
      if (!entry || typeof entry !== "object") return [];
      const fields = rows([["搜索词", entry.q], ["允许的域名", entry.domains], ["最近天数", entry.recency]]);
      return fields.length ? [fields] : [];
    });
  });
  const extra = rows([["返回篇幅", options.response_length]]);
  if (extra.length) groups.push(extra);
  return groups;
}

/** `exec` 脚本里一次或多次网页搜索的参数。 */
export function webSearchesView(searches: { tool: string; queries: string[]; input?: unknown }[]): View | undefined {
  const groups = searches.flatMap(({ tool, queries, input }) => searchGroups(tool, queries, input));
  if (!groups.length) return;
  const copy = groups.flatMap(entries => entries.map(([, values]) => values.join("\n"))).join("\n");
  return { label: "查询", copy, body: <ExpandableFrame label="查询">{() => <div className="web-search-batch">
    {groups.map((entries, i) => <WebFields key={i} entries={entries} />)}
  </div>}</ExpandableFrame> };
}

export function webSearchHead(op: SearchOp, badges: Badges = []): Head {
  const [first] = op.queries;
  return { icon: Globe, label: "网页搜索", head: <>
    <span className="op-title" title={op.queries.join("\n") || undefined}>{first ?? "网页搜索"}</span>
    <span className="op-meta">
      {op.queries.length > 1 && <Chip tone="sans">{op.queries.length} 条查询</Chip>}
      {op.domain && <Chip>{op.domain}</Chip>}
      {op.results !== undefined && <Chip tone="sans">{op.results} 条结果</Chip>}
    </span>
    <Stats items={badges} />
  </> };
}

export function WebSearchCard({ item, op }: { item: ToolItem; op: SearchOp }) {
  const { icon, label, head } = webSearchHead(op);
  return <OpCard item={item} className="web_search" icon={icon} label={label} head={head}>
    <ToolSections<SearchOp> item={item} input={{ views: (op, call) => [webSearchView(op.queries, call.input, op.domain)] }}
      output={{ title: "结果", views: result => [webResult(result)], preferScript: true }} />
  </OpCard>;
}

export function webFetchHead(op: FetchOp, find: boolean, badges: Badges = []): Head {
  const label = find ? "在网页中查找" : "读取网页";
  const target = (find && op.prompt) || op.url || label;
  return { icon: Globe, label, head: <>
    {find ? <span className="op-title" title={target}>{target}</span>
      : <code className="op-name" title={target}>{target.replace(/^https?:\/\//, "")}</code>}
    <Stats items={badges} />
  </> };
}

export function WebFetchCard({ item, op }: { item: ToolItem; op: FetchOp }) {
  const find = !!op.find;
  const { icon, label, head } = webFetchHead(op, find);
  return <OpCard item={item} className="web_fetch" icon={icon} label={label} head={head}>
    <ToolSections<FetchOp> item={item} input={{ views: (op, call) => [
      fieldsView("目标", [["网址", op.url], [find ? "查找词" : "提问", op.prompt],
        ["格式", (call.input as { format?: unknown } | undefined)?.format]]),
    ] }} output={{ title: find ? "结果" : "内容", views: result => [webResult(result, find ? "find" : "fetch")], preferScript: true }} />
  </OpCard>;
}
