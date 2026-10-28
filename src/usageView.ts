import type { UsageReport, UsageRow, UsageTokens, UsageSummaryQuery, UsageSummaryRow, UsageRequestMetrics } from "./types";

export type UsagePeriod = "today" | "week" | "month" | "all";
export const emptyTokens = (): UsageTokens => ({ inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, reasoningTokens: 0, unclassifiedTokens: 0, totalTokens: 0, inputRecorded: false, outputRecorded: false, inputIncomplete: false, outputIncomplete: false, cacheReadRecorded: false, cacheWriteRecorded: false, reasoningRecorded: false });
export function addTokens(target: UsageTokens, row: UsageTokens) {
  // 分组对象还带有模型/日期等字段，只累加明确的计量项。
  for (const key of ["inputTokens", "outputTokens", "cacheReadTokens", "cacheWriteTokens", "reasoningTokens", "unclassifiedTokens", "totalTokens"] as const) target[key] += row[key];
  for (const key of ["inputRecorded", "outputRecorded", "inputIncomplete", "outputIncomplete", "cacheReadRecorded", "cacheWriteRecorded", "reasoningRecorded"] as const) target[key] = !!target[key] || !!row[key];
  return target;
}
export function formatUsageField(tokens: UsageTokens, field: "input" | "output") {
  const recorded = field === "input" ? tokens.inputRecorded : tokens.outputRecorded;
  if (!recorded) return "未提供";
  const incomplete = field === "input" ? tokens.inputIncomplete : tokens.outputIncomplete;
  return `${incomplete ? "≥ " : ""}${formatTokens(field === "input" ? tokens.inputTokens : tokens.outputTokens)}`;
}
export function dateKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}
export function firstDate(period: UsagePeriod, now: Date): string {
  if (period === "all") return "";
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  start.setDate(start.getDate() - (period === "week" ? 6 : period === "month" ? 29 : 0));
  return dateKey(start);
}
export function selectRows(report: UsageReport | undefined, period: UsagePeriod, model: string, now: Date): UsageRow[] {
  const start = firstDate(period, now), end = dateKey(now);
  return (report?.rows ?? []).filter(row => (period === "all" || !!row.date && row.date >= start && row.date <= end) && (model === "all" || row.model === model));
}
export function sumTokens(rows: UsageTokens[]): UsageTokens { return rows.reduce(addTokens, emptyTokens()); }
export function dailySeries(rows: UsageRow[], period: UsagePeriod, now: Date): (UsageTokens & { date: string })[] {
  const dates = rows.map(r => r.date).filter(Boolean).sort();
  const start = firstDate(period, now) || dates[0] || dateKey(now);
  const end = dateKey(now);
  const byDay = new Map<string, UsageTokens>();
  for (const row of rows) if (row.date) byDay.set(row.date, addTokens(byDay.get(row.date) ?? emptyTokens(), row));
  const cursor = new Date(`${start}T12:00:00`), result: (UsageTokens & { date: string })[] = [];
  // 使用本地日期递增，夏令时不能按固定 24 小时递增。
  while (dateKey(cursor) <= end && result.length < 36525) {
    const date = dateKey(cursor); result.push({ date, ...(byDay.get(date) ?? emptyTokens()) }); cursor.setDate(cursor.getDate() + 1);
  }
  return result;
}
export interface ModelUsage extends UsageTokens, UsageRequestMetrics { partial: boolean }
type ModelUsageRow = UsageRow & Partial<UsageRequestMetrics>;
const emptyModelUsage = (): ModelUsage => ({ ...emptyTokens(), partial: false, requestCount: 0, requestCountIncomplete: false, costUsd: null, costIncomplete: false, cacheReadIncomplete: false, cacheRateUnavailable: false });
function addModelUsage(target: ModelUsage, row: ModelUsageRow) {
  addTokens(target, row);
  target.partial ||= row.partial;
  target.requestCount += row.requestCount ?? 0;
  target.requestCountIncomplete ||= row.requestCountIncomplete ?? true;
  if (row.costUsd != null) target.costUsd = (target.costUsd ?? 0) + row.costUsd;
  target.costIncomplete ||= row.costIncomplete ?? true;
  target.cacheReadIncomplete ||= row.cacheReadIncomplete ?? true;
  target.cacheRateUnavailable ||= row.cacheRateUnavailable ?? true;
  return target;
}
export function sumModelUsage(rows: ModelUsageRow[]): ModelUsage { return rows.reduce(addModelUsage, emptyModelUsage()); }
export function modelTotals(rows: ModelUsageRow[]) {
  const models = new Map<string, ModelUsage & { model: string; modelName: string }>();
  for (const row of rows) {
    const value = models.get(row.model) ?? { ...emptyModelUsage(), model: row.model, modelName: row.modelName };
    addModelUsage(value, row); models.set(row.model, value);
  }
  return [...models.values()].sort((a, b) => b.totalTokens - a.totalTokens || a.model.localeCompare(b.model));
}
const compactNumber = (value: number) => new Intl.NumberFormat("en-US", { notation: "compact", maximumFractionDigits: 2 }).format(value);
export function modelUsageCells(row: ModelUsage): { text: string; title: string }[] {
  const count = (value: number, recorded: boolean | undefined, incomplete: boolean | undefined, unit: string) => ({
    text: recorded ? `${incomplete ? "≥ " : ""}${compactNumber(value)}` : "—",
    title: recorded ? `${incomplete ? "已记录下限：" : ""}${formatTokens(value)} ${unit}` : "未记录",
  });
  // 对合计后的分子/分母求比值，不能平均各请求或各模型的百分比。
  const rate = !row.cacheRateUnavailable && row.inputRecorded && row.cacheReadRecorded && !row.inputIncomplete && row.inputTokens > 0 && row.cacheReadTokens <= row.inputTokens
    ? row.cacheReadTokens / row.inputTokens * 100 : null;
  const cost = row.costUsd == null ? "—" : `${row.costIncomplete ? "≥ " : ""}${new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 6 }).format(row.costUsd)}`;
  return [
    count(row.requestCount, row.requestCount > 0 || !row.requestCountIncomplete, row.requestCountIncomplete, "次请求"),
    count(row.inputTokens, row.inputRecorded, row.inputIncomplete, "Token"),
    count(row.outputTokens, row.outputRecorded, row.outputIncomplete, "Token"),
    count(row.cacheReadTokens, row.cacheReadRecorded, row.cacheReadIncomplete, "Token"),
    { text: rate == null ? "—" : `${rate.toFixed(2)}%`, title: rate == null ? "输入或缓存记录不足，无法计算" : "缓存读取 ÷ 输入（输入含缓存）" },
    { text: cost, title: row.costUsd == null ? "日志未记录成本" : `${row.costIncomplete ? "仅部分请求记录成本；" : ""}已记录 ${row.costUsd.toFixed(6)} USD` },
  ];
}
export function formatTokens(value: number, compact = false) {
  return new Intl.NumberFormat("zh-CN", compact ? { notation: "compact", maximumFractionDigits: 1 } : { maximumFractionDigits: 0 }).format(value);
}

export function summaryQuery(period: UsagePeriod, now: Date): UsageSummaryQuery {
  if (period === "all") return { bounds: [] };
  const start = new Date(`${firstDate(period, now)}T00:00:00`);
  const end = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
  const bounds: number[] = [];
  for (const cursor = new Date(start); cursor < end;) {
    bounds.push(cursor.getTime());
    // 小时按真实经过时间递增，保留秋季重复的一小时；日按本地日历递增。
    if (period === "today") cursor.setTime(cursor.getTime() + 3600000);
    else cursor.setDate(cursor.getDate() + 1);
  }
  bounds.push(end.getTime());
  return { from: start.getTime(), to: end.getTime(), bounds };
}
export interface UsagePoint extends UsageTokens { key: string; label: string; title: string }
export function usageSeries(rows: UsageSummaryRow[], period: UsagePeriod, now: Date): UsagePoint[] {
  if (period !== "today") return dailySeries(rows, period, now).map(d => ({ ...d, key: d.date, label: d.date.slice(5).replace("-", "/"), title: d.date }));
  const bounds = summaryQuery(period, now).bounds;
  const buckets = new Map<number, UsageTokens>();
  for (const row of rows) if (row.bucketStart != null) buckets.set(row.bucketStart, addTokens(buckets.get(row.bucketStart) ?? emptyTokens(), row));
  const labels = bounds.slice(0, -1).map(t => new Date(t).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false }));
  return bounds.slice(0, -1).map((start, i) => {
    const duplicate = labels.filter(l => l === labels[i]).length > 1;
    const offset = new Date(start).toLocaleTimeString("zh-CN", { timeZoneName: "shortOffset", hour: "2-digit", minute: "2-digit", hour12: false });
    return { ...emptyTokens(), ...buckets.get(start), key: String(start), label: labels[i], title: `${dateKey(new Date(start))} ${duplicate ? offset : labels[i]}–${new Date(bounds[i + 1]).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false })}` };
  });
}
