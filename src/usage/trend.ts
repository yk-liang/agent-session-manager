import type { RequestOutcome, UsageSummaryRow } from "./types.ts";
import { formatHourMinute } from "./format.ts";
import { dateKey, firstDate, summaryQuery, type UsagePeriod } from "./range.ts";
import { addModelUsage, emptyModelUsage, groupUsageBy, metricValue, OTHERS_KEY, uncachedInputOf, type ModelUsage, type ModelUsageRow, type TrendMetric } from "./metrics.ts";

export type TrendGroup = "category" | "models" | "agents";

export function dailySeries(
  rows: (ModelUsageRow & { date: string })[],
  period: UsagePeriod,
  now: Date,
): (ModelUsage & { date: string })[] {
  const dates = rows.map(r => r.date).filter(Boolean).sort();
  const start = firstDate(period, now) || dates[0] || dateKey(now);
  const end = dateKey(now);
  const byDay = new Map<string, ModelUsage>();
  for (const row of rows) {
    if (row.date) byDay.set(row.date, addModelUsage(byDay.get(row.date) ?? emptyModelUsage(), row));
  }
  const cursor = new Date(`${start}T12:00:00`);
  const result: (ModelUsage & { date: string })[] = [];
  // 按本地日期推进，避开夏令时的 23 和 25 小时日。
  while (dateKey(cursor) <= end && result.length < 36525) {
    const date = dateKey(cursor);
    result.push({ date, ...(byDay.get(date) ?? emptyModelUsage()) });
    cursor.setDate(cursor.getDate() + 1);
  }
  return result;
}

export interface UsagePoint extends ModelUsage {
  key: string;
  /** 坐标轴短标签。 */
  label: string;
  /** 提示框标题：完整日期或时段。 */
  title: string;
}

const WEEKDAYS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

export function usageSeries(rows: UsageSummaryRow[], period: UsagePeriod, now: Date): UsagePoint[] {
  if (period !== "today") {
    return dailySeries(rows, period, now).map(d => {
      const [year, month, day] = d.date.split("-").map(Number);
      const weekday = WEEKDAYS[new Date(year, month - 1, day).getDay()];
      return { ...d, key: d.date, label: `${month}/${day}`, title: `${year}年${month}月${day}日 ${weekday}` };
    });
  }
  const bounds = summaryQuery(period, now).bounds;
  const buckets = new Map<number, ModelUsage>();
  for (const row of rows) {
    if (row.bucketStart != null) buckets.set(row.bucketStart, addModelUsage(buckets.get(row.bucketStart) ?? emptyModelUsage(), row));
  }
  const starts = bounds.slice(0, -1);
  const labels = starts.map(formatHourMinute);
  return starts.map((start, i) => {
    // 秋季重复的一小时用时区偏移区分。
    const duplicate = labels.indexOf(labels[i]) !== labels.lastIndexOf(labels[i]);
    const startLabel = duplicate
      ? new Date(start).toLocaleTimeString("zh-CN", { timeZoneName: "shortOffset", hour: "2-digit", minute: "2-digit", hour12: false })
      : labels[i];
    return {
      ...emptyModelUsage(),
      ...buckets.get(start),
      key: String(start),
      label: labels[i],
      title: `${dateKey(new Date(start))} ${startLabel}–${formatHourMinute(bounds[i + 1])}`,
    };
  });
}

const TABLEAU_10 = [
  "#4e79a7", "#f28e2c", "#e15759", "#76b7b2", "#59a14f",
  "#edc949", "#af7aa1", "#ff9da7", "#9c755f", "#bab0ab",
] as const;
export const OTHER_COLOR = TABLEAU_10[9];

/** 实体颜色跟随实体：按当前范围总 Token 排序固定，切换指标或图表时不变。 */
export function entityColors(keys: string[]): Map<string, string> {
  return new Map(keys.map((key, i) => [key, TABLEAU_10[i % TABLEAU_10.length]]));
}

const TOKEN_COLORS = {
  input: TABLEAU_10[0],
  cacheRead: TABLEAU_10[1],
  cacheWrite: TABLEAU_10[6],
  output: TABLEAU_10[4],
  unclassified: OTHER_COLOR,
};
const COST_COLORS = { custom: TABLEAU_10[1], reference: TABLEAU_10[6] };
export const REQUEST_OUTCOMES = {
  success: { label: "成功", color: TABLEAU_10[4] },
  failed: { label: "失败", color: TABLEAU_10[2] },
} satisfies Record<RequestOutcome, { label: string; color: string }>;

export const REQUEST_OUTCOME_IDS = Object.keys(REQUEST_OUTCOMES) as RequestOutcome[];

export interface StackedSeries {
  key: string;
  label: string;
  color: string;
  values: number[];
}

export function buildTrendSeries(
  rows: UsageSummaryRow[],
  points: UsagePoint[],
  period: UsagePeriod,
  metric: TrendMetric,
  group: TrendGroup,
  labelFor: (dimension: "models" | "agents", key: string) => string,
  colors?: Map<string, string>,
): StackedSeries[] {
  const nonEmpty = (series: StackedSeries[]) => series.filter(s => s.values.some(v => v > 0));
  const make = (key: string, label: string, color: string, value: (p: UsagePoint) => number): StackedSeries => ({
    key, label, color, values: points.map(value),
  });
  if (group === "category") {
    if (metric === "tokens") {
      // 只有每个时段的缓存读写都包含在输入内时，才拆出未缓存输入，避免重复或负值。
      const split = points.every(p => p.cacheReadTokens + p.cacheWriteTokens === 0 || uncachedInputOf(p) != null);
      return nonEmpty(
        split
          ? [
              make("uncached_input", "未缓存输入", TOKEN_COLORS.input, p => uncachedInputOf(p) ?? p.inputTokens),
              make("cache_read", "缓存读取", TOKEN_COLORS.cacheRead, p => p.cacheReadTokens),
              make("cache_write", "缓存写入", TOKEN_COLORS.cacheWrite, p => p.cacheWriteTokens),
              make("output", "输出", TOKEN_COLORS.output, p => p.outputTokens),
              make("unclassified", "未分项", TOKEN_COLORS.unclassified, p => p.unclassifiedTokens),
            ]
          : [
              make("input", "输入（含缓存）", TOKEN_COLORS.input, p => p.inputTokens),
              make("output", "输出", TOKEN_COLORS.output, p => p.outputTokens),
              make("unclassified", "未分项", TOKEN_COLORS.unclassified, p => p.unclassifiedTokens),
            ],
      );
    }
    if (metric === "requests") {
      const { success, failed } = REQUEST_OUTCOMES;
      return nonEmpty([
        make("success", success.label, success.color, p => p.successCount),
        make("failed", failed.label, failed.color, p => p.failedCount),
      ]);
    }
    return nonEmpty([
      make("custom", "自定义价格估算", COST_COLORS.custom, p => p.customCostUsd ?? 0),
      make("reference", "参考价估算", COST_COLORS.reference, p => p.referenceCostUsd ?? 0),
    ]);
  }

  // 按模型或 Agent 分组：前六项单列，其余合并为“其他”。
  const groups = groupUsageBy(rows, group);
  const palette = colors ?? entityColors(groups.map(row => row.key));
  groups.sort(
    (a, b) => metricValue(b, metric) - metricValue(a, metric) || b.totalTokens - a.totalTokens,
  );
  const keepAll = group === "agents";
  const top = keepAll || groups.length <= 7 ? groups : groups.slice(0, 6);
  const index = new Map(top.map((item, i) => [item.key, i]));
  const series: StackedSeries[] = top.map(item => ({
    key: item.key,
    label: labelFor(group, item.key),
    color: palette.get(item.key) ?? OTHER_COLOR,
    values: Array(points.length).fill(0),
  }));
  if (top.length < groups.length) {
    series.push({ key: OTHERS_KEY, label: `其他 ${groups.length - top.length} 项`, color: OTHER_COLOR, values: Array(points.length).fill(0) });
  }
  const column = new Map(points.map((p, i) => [p.key, i]));
  for (const row of rows) {
    const col = column.get(period === "today" ? (row.bucketStart != null ? String(row.bucketStart) : "") : row.date);
    if (col === undefined) continue;
    const i = index.get(group === "models" ? row.model : row.agent) ?? (top.length < groups.length ? series.length - 1 : -1);
    if (i < 0) continue;
    series[i].values[col] += metric === "requests" ? row.requestCount : metric === "cost" ? row.costUsd ?? 0 : row.totalTokens;
  }
  return nonEmpty(series);
}
