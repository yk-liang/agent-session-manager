export type UsagePeriod = "today" | "week" | "month" | "all";

export function dateKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function firstDate(period: UsagePeriod, now: Date): string {
  if (period === "all") return "";
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  start.setDate(start.getDate() - (period === "week" ? 6 : period === "month" ? 29 : 0));
  return dateKey(start);
}

export function summaryQuery(period: UsagePeriod, now: Date) {
  if (period === "all") return { bounds: [] as number[] };
  const start = new Date(`${firstDate(period, now)}T00:00:00`);
  const end = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
  const bounds: number[] = [];
  for (const cursor = new Date(start); cursor < end; ) {
    bounds.push(cursor.getTime());
    // 小时按真实经过时间递增，保留秋季重复的一小时；日按本地日历递增。
    if (period === "today") cursor.setTime(cursor.getTime() + 3600000);
    else cursor.setDate(cursor.getDate() + 1);
  }
  bounds.push(end.getTime());
  return { from: start.getTime(), to: end.getTime(), bounds };
}
