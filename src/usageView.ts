import type { UsageReport, UsageRow, UsageTokens, UsageSummaryQuery, UsageSummaryRow } from "./types";

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
export function modelTotals(rows: UsageRow[]) {
  const models = new Map<string, UsageTokens & { model: string; modelName: string; partial: boolean }>();
  for (const row of rows) {
    const value = models.get(row.model) ?? { ...emptyTokens(), model: row.model, modelName: row.modelName, partial: false };
    addTokens(value, row); value.partial ||= row.partial; models.set(row.model, value);
  }
  return [...models.values()].sort((a, b) => b.totalTokens - a.totalTokens || a.model.localeCompare(b.model));
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
