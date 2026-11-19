import type { DimensionSummaryRow, EventCost, PricingIssue, RequestEvent, RequestMetadata, UsageRequestMetrics, UsageRow, UsageSummaryRow, UsageTokens } from "./types.ts";
import { display, formatCount, formatPercent, formatUsd, type Display } from "./format.ts";

export type TrendMetric = "tokens" | "requests" | "cost";
export type AnalysisDimension = "models" | "agents" | "projects" | "providers";

const TOKEN_KEYS = [
  "inputTokens",
  "outputTokens",
  "cacheReadTokens",
  "cacheWriteTokens",
  "cacheWrite1hTokens",
  "reasoningTokens",
  "unclassifiedTokens",
  "totalTokens",
] as const;
const FLAG_KEYS = [
  "inputRecorded",
  "outputRecorded",
  "inputIncomplete",
  "outputIncomplete",
  "cacheReadRecorded",
  "cacheWriteRecorded",
  "reasoningRecorded",
] as const;

export const emptyTokens = (): UsageTokens => ({
  inputTokens: 0,
  outputTokens: 0,
  cacheReadTokens: 0,
  cacheWriteTokens: 0,
  cacheWrite1hTokens: 0,
  reasoningTokens: 0,
  unclassifiedTokens: 0,
  totalTokens: 0,
  inputRecorded: false,
  outputRecorded: false,
  inputIncomplete: false,
  outputIncomplete: false,
  cacheReadRecorded: false,
  cacheWriteRecorded: false,
  reasoningRecorded: false,
});

export function addTokens(target: UsageTokens, row: UsageTokens) {
  for (const key of TOKEN_KEYS) target[key] = (target[key] ?? 0) + (row[key] ?? 0);
  for (const key of FLAG_KEYS) target[key] = !!target[key] || !!row[key];
  return target;
}

export function sumTokens(rows: UsageTokens[]): UsageTokens {
  return rows.reduce(addTokens, emptyTokens());
}

export function requestRecordCounts(rows: UsageSummaryRow[]) {
  const counts = new Map<string, number | null>();
  let total: number | null = 0;
  for (const row of rows) {
    const key = row.agent;
    const previous = counts.get(key);
    const count = row.recordCount;
    counts.set(key, count == null || previous === null ? null : (previous ?? 0) + count);
    total = count == null || total === null ? null : total + count;
  }
  return { counts, total };
}

export interface ModelUsage extends UsageTokens, UsageRequestMetrics {
  recordCount: number;
  successCount: number;
  failedCount: number;
  customCostUsd: number | null;
  referenceCostUsd: number | null;
  costUnknownCount: number;
  unpricedCount: number;
  costUsageIncompleteCount: number;
  costSpecialCount: number;
  latencySumMs: number;
  latencyCount: number;
  tpsSum: number;
  tpsCount: number;
}

export type ModelUsageRow = UsageTokens &
  Partial<UsageRequestMetrics> & {
    recordCount?: number;
  };

export const emptyModelUsage = (): ModelUsage => ({
  ...emptyTokens(),
  recordCount: 0,
  requestCount: 0,
  requestCountIncomplete: false,
  successCount: 0,
  failedCount: 0,
  costUsd: null,
  customCostUsd: null,
  referenceCostUsd: null,
  costUnknownCount: 0,
  unpricedCount: 0,
  costUsageIncompleteCount: 0,
  costSpecialCount: 0,
  costIncomplete: false,
  cacheReadIncomplete: false,
  cacheRateUnavailable: false,
  latencySumMs: 0,
  latencyCount: 0,
  tpsSum: 0,
  tpsCount: 0,
});

const SUMMED = [
  "requestCount",
  "successCount",
  "failedCount",
  "costUnknownCount",
  "unpricedCount",
  "costUsageIncompleteCount",
  "costSpecialCount",
  "latencySumMs",
  "latencyCount",
  "tpsSum",
  "tpsCount",
] as const;
const NULLABLE = ["costUsd", "customCostUsd", "referenceCostUsd"] as const;

export function addModelUsage(target: ModelUsage, row: ModelUsageRow): ModelUsage {
  addTokens(target, row);
  target.recordCount += row.recordCount ?? 0;
  for (const key of SUMMED) target[key] += row[key] ?? 0;
  // 全部未知时保留 null，避免将缺失费用显示为零。
  for (const key of NULLABLE) {
    const value = row[key];
    if (value != null && Number.isFinite(value)) target[key] = (target[key] ?? 0) + value;
  }
  // 旧记录缺少完整性标记时按下限处理。
  target.requestCountIncomplete ||= row.requestCountIncomplete ?? true;
  target.costIncomplete ||= row.costIncomplete ?? true;
  target.cacheReadIncomplete ||= row.cacheReadIncomplete ?? true;
  target.cacheRateUnavailable ||= row.cacheRateUnavailable ?? true;
  return target;
}

export function sumModelUsage(rows: ModelUsageRow[]): ModelUsage {
  return rows.reduce(addModelUsage, emptyModelUsage());
}

export interface GroupedUsage extends ModelUsage {
  key: string;
}

function groupBy<T extends ModelUsageRow>(rows: T[], keyOf: (row: T) => string | null): GroupedUsage[] {
  const map = new Map<string, GroupedUsage>();
  for (const row of rows) {
    const key = keyOf(row);
    if (key == null) continue;
    const entry = map.get(key) ?? { ...emptyModelUsage(), key };
    addModelUsage(entry, row);
    map.set(key, entry);
  }
  return [...map.values()].sort((a, b) => b.totalTokens - a.totalTokens || a.key.localeCompare(b.key));
}

export function modelTotals(rows: (UsageRow & Partial<UsageRequestMetrics> & { recordCount?: number })[]) {
  return groupBy(rows, row => row.model);
}

export function groupUsageBy(rows: UsageSummaryRow[], dimension: "models" | "agents"): GroupedUsage[] {
  return dimension === "models" ? modelTotals(rows) : groupBy(rows, row => row.agent);
}

export function dimensionTotals(rows: DimensionSummaryRow[] | undefined, agent: string): GroupedUsage[] {
  return groupBy(rows ?? [], row => agent === "all" || row.agent === agent ? row.key || "" : null);
}

export interface DistributionSlice extends GroupedUsage {
  othersCount?: number;
  share: number;
  value: number;
}

export function metricValue(row: ModelUsage, metric: TrendMetric): number {
  if (metric === "requests") return row.requestCount;
  if (metric === "cost") return row.costUsd ?? 0;
  return row.totalTokens;
}

export const OTHERS_KEY = "__others__";

/** 前 maxTop 项单列，其余合并为“其他”；只多出一项时不合并。 */
export function distributionSlices(rows: GroupedUsage[], metric: TrendMetric, maxTop = 6, keepAll = false): DistributionSlice[] {
  const sorted = [...rows].sort(
    (a, b) => metricValue(b, metric) - metricValue(a, metric) || b.totalTokens - a.totalTokens || a.key.localeCompare(b.key),
  );
  const fold = !keepAll && sorted.length > maxTop + 1;
  const items: Array<GroupedUsage & { othersCount?: number }> = fold ? sorted.slice(0, maxTop) : sorted;
  if (fold) items.push({ ...sumModelUsage(sorted.slice(maxTop)), key: OTHERS_KEY, othersCount: sorted.length - maxTop });
  const total = items.reduce((sum, item) => sum + metricValue(item, metric), 0);
  return items.map(item => {
    const value = metricValue(item, metric);
    return { ...item, value, share: total > 0 ? (value / total) * 100 : 0 };
  });
}

export function cacheRateOf(row: ModelUsage): number | null {
  return !row.cacheRateUnavailable &&
    row.inputRecorded &&
    row.cacheReadRecorded &&
    !row.inputIncomplete &&
    row.inputTokens > 0 &&
    row.cacheReadTokens <= row.inputTokens
    ? (row.cacheReadTokens / row.inputTokens) * 100
    : null;
}

/** 输入已包含缓存；只有缓存读写都在输入之内时才能得出未缓存输入。 */
type TokenFields = Omit<UsageTokens, "totalTokens">;

export function uncachedInputOf(row: TokenFields & { cacheRateUnavailable?: boolean }): number | null {
  return row.inputRecorded &&
    !row.inputIncomplete &&
    row.cacheReadRecorded &&
    !row.cacheRateUnavailable &&
    row.cacheReadTokens + row.cacheWriteTokens <= row.inputTokens
    ? row.inputTokens - row.cacheReadTokens - row.cacheWriteTokens
    : null;
}

export function tokenDisplay(
  tokens: TokenFields,
  field: "input" | "output" | "cacheRead" | "cacheWrite" | "reasoning",
  options: { incomplete?: boolean; compact?: boolean } = {},
): Display {
  const recorded = {
    input: tokens.inputRecorded,
    output: tokens.outputRecorded,
    cacheRead: tokens.cacheReadRecorded,
    cacheWrite: tokens.cacheWriteRecorded,
    reasoning: tokens.reasoningRecorded,
  }[field];
  const value = {
    input: tokens.inputTokens,
    output: tokens.outputTokens,
    cacheRead: tokens.cacheReadTokens,
    cacheWrite: tokens.cacheWriteTokens,
    reasoning: tokens.reasoningTokens,
  }[field];
  const incomplete =
    options.incomplete ?? (field === "input" ? tokens.inputIncomplete : field === "output" ? tokens.outputIncomplete : false);
  return display(recorded ? value : null, {
    compact: options.compact ?? true,
    exactness: incomplete ? "lowerBound" : "exact",
    unit: "Token",
    missingTitle: "未记录此项用量",
  });
}

export function requestDisplay(row: ModelUsage): Display {
  const known = row.requestCount > 0 || !row.requestCountIncomplete;
  return display(known ? row.requestCount : null, {
    exactness: row.requestCountIncomplete ? "lowerBound" : "exact",
    unit: "次请求",
    missingTitle: "累计用量差额仅计入 Token 用量",
  });
}

export function successRateDisplay({ successCount, failedCount }: ModelUsage): string {
  const attempts = successCount + failedCount;
  if (!attempts) return "—";
  if (failedCount === 0) return "100%";
  if (successCount === 0) return "0%";
  return `${((successCount / attempts) * 100).toFixed(2)}%`;
}

export function requestErrorText({ httpStatus, error }: RequestMetadata): string {
  const heading = [httpStatus ? `HTTP ${httpStatus}` : "", error?.kind].filter(Boolean).join(" ");
  const message = error?.message === error?.kind ? "" : error?.message;
  return [heading, message].filter(Boolean).join("\n");
}

export function costDisplay(row: ModelUsage): Display {
  if (row.costUsd == null || !Number.isFinite(row.costUsd)) {
    return display(null, { missingTitle: costGapNotes(row).join("；") || "费用未知" });
  }
  const shown = display(row.costUsd, {
    kind: "usd",
    exactness: row.costIncomplete ? "lowerBound" : "exact",
  });
  const parts = [
    row.customCostUsd != null && `自定义价格 ${formatUsd(row.customCostUsd)}`,
    row.referenceCostUsd != null && `参考价 ${formatUsd(row.referenceCostUsd)}`,
    ...costGapNotes(row),
  ].filter(Boolean);
  return { ...shown, title: parts.join("；") || shown.title };
}

export function costGapNotes(row: ModelUsage): string[] {
  const other = Math.max(0, row.costUnknownCount - row.unpricedCount - row.costUsageIncompleteCount - row.costSpecialCount);
  return [
    row.unpricedCount > 0 && `${formatCount(row.unpricedCount)} 次请求的模型未定价，未计入费用`,
    row.costUsageIncompleteCount > 0 && `${formatCount(row.costUsageIncompleteCount)} 次请求缺少输入或输出 Token，无法估算`,
    row.costSpecialCount > 0 && `${formatCount(row.costSpecialCount)} 次请求缺少适用的价格`,
    other > 0 && `${formatCount(other)} 次请求费用未知`,
  ].filter((note): note is string => !!note);
}

export function cacheRateDisplay(row: ModelUsage): Display {
  const rate = cacheRateOf(row);
  return rate == null
    ? display(null, { missingTitle: "输入或缓存读取数据不完整，无法计算命中率" })
    : { text: formatPercent(rate), prefix: "", missing: false, title: "缓存读取 Token ÷ 输入 Token" };
}

export function groupUsageCells(row: ModelUsage): Display[] {
  return [
    tokenDisplay(row, "input"),
    tokenDisplay(row, "output"),
    tokenDisplay(row, "cacheRead", { incomplete: row.cacheReadIncomplete }),
    cacheRateDisplay(row),
    costDisplay(row),
  ];
}

const SOURCE_NAMES: Record<string, string> = { "models.dev": "Models.dev", litellm: "LiteLLM", custom: "自定义价格", builtin: "内置" };
export const sourceName = (source: string) => SOURCE_NAMES[source] ?? source;

export const PRICING_ISSUES: Record<PricingIssue, { label: string; detail: string }> = {
  usage_incomplete: { label: "缺少 Token", detail: "缺少完整的输入或输出 Token，无法估算费用" },
  unclassified: { label: "仅记录总量", detail: "仅记录了 Token 总量，无法估算费用" },
  cache_ambiguous: { label: "缓存归属未知", detail: "无法确认输入是否包含缓存，无法估算费用" },
  unpriced: { label: "未定价", detail: "该模型没有可用价格" },
  unsupported_tier: { label: "档位未定价", detail: "该服务档位没有可用价格" },
  unsupported_component: { label: "分项未定价", detail: "部分计费项没有可用价格" },
};

const TIER_NAMES: Record<string, string> = { priority: "优先处理", flex: "Flex", batch: "批处理" };
export const tierName = (tier: string) => TIER_NAMES[tier] ?? tier;

export function eventCostDisplay(event: RequestEvent): Display & { note: string } {
  const cost = event.cost;
  if (!cost || !Number.isFinite(cost.totalUsd)) {
    const issue = event.pricingIssue ? PRICING_ISSUES[event.pricingIssue] : null;
    return { ...display(null, { missingTitle: issue?.detail ?? "费用未知" }), note: issue?.label ?? "未知" };
  }
  const shown = display(cost.totalUsd, { kind: "usd" });
  if (cost.source === "builtin") return { ...shown, title: "免费模型（内置价格）", note: sourceName(cost.source) };
  const notes = costNotes(cost);
  return {
    ...shown,
    title: [priceSourceText(cost), ...notes].join(" · "),
    note: [cost.source === "custom" ? "自定义价格" : "参考价", ...notes].join(" · "),
  };
}

function priceSourceText(cost: Pick<EventCost, "source" | "currency">): string {
  return cost.source === "custom"
    ? `自定义价格（${cost.currency === "CNY" ? "CNY，1 USD = 7 CNY" : "USD"}）`
    : `${sourceName(cost.source)} 参考价`;
}

function costNotes(cost: EventCost): string[] {
  return [
    cost.longContext ? "长上下文" : "",
    cost.serviceTier ? `${tierName(cost.serviceTier)} 档` : "",
    cost.multiplier && cost.multiplier !== 1 ? `标准价 × ${cost.multiplier}` : "",
  ].filter(Boolean);
}

/** 单次请求费用的分项：[名称, Token 数, 单价/百万, 金额]。 */
export function costBreakdown(event: RequestEvent): [string, number, number | null | undefined, number | null | undefined][] {
  const cost = event.cost;
  if (!cost || cost.source === "builtin") return [];
  const write1h = Math.min(event.cacheWrite1hTokens ?? 0, event.cacheWriteTokens);
  const rows: [string, number, number | null | undefined, number | null | undefined][] = [
    ["未缓存输入", Math.max(0, event.inputTokens - event.cacheReadTokens - event.cacheWriteTokens), cost.inputRate, cost.inputUsd],
    ["缓存读取", event.cacheReadTokens, cost.cacheReadRate, cost.cacheReadUsd],
    [write1h > 0 ? "缓存写入（5 分钟）" : "缓存写入", event.cacheWriteTokens - write1h, cost.cacheWriteRate, cost.cacheWriteUsd],
    ["输出", event.outputTokens, cost.outputRate, cost.outputUsd],
  ];
  if (write1h > 0) rows.splice(3, 0, ["缓存写入（1 小时）", write1h, cost.cacheWrite1hRate, cost.cacheWrite1hUsd]);
  return rows.filter(([label, tokens]) => tokens > 0 || label === "未缓存输入" || label === "输出");
}
