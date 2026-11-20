import type { PricingCurrency } from "./types";

export const LOCALE = "zh-CN";
export const CNY_PER_USD = 7;

export interface Display {
  text: string;
  /** 前缀符号单独返回，便于界面弱化显示，读屏仍按完整文本朗读。 */
  prefix: "" | "≥";
  missing: boolean;
  title: string;
}

const integer = new Intl.NumberFormat(LOCALE, { maximumFractionDigits: 0 });
const tokenNumber = new Intl.NumberFormat(LOCALE, { maximumFractionDigits: 2 });
const oneDecimal = new Intl.NumberFormat(LOCALE, { minimumFractionDigits: 1, maximumFractionDigits: 1 });

const money = (currency: PricingCurrency, options: Intl.NumberFormatOptions) =>
  new Intl.NumberFormat(LOCALE, { style: "currency", currency, currencyDisplay: "narrowSymbol", ...options });
const usdCents = money("USD", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const usdSmall = money("USD", { minimumFractionDigits: 2, maximumFractionDigits: 4 });
const usdTiny = money("USD", { maximumSignificantDigits: 2 });
const rateDigits = { minimumFractionDigits: 0, maximumFractionDigits: 4 };
const rates = { USD: money("USD", rateDigits), CNY: money("CNY", rateDigits) };
const TINY_USD = 0.000001;

export function formatCount(value: number): string {
  return Number.isFinite(value) ? integer.format(Math.round(value)) : "—";
}

export function formatTokens(value: number): string {
  if (!Number.isFinite(value)) return "—";
  const units = ["", "K", "M", "B"];
  let scale = 0;
  while (Math.abs(value) >= 999.995 && scale < units.length - 1) {
    value /= 1000;
    scale++;
  }
  return `${scale ? tokenNumber.format(value) : formatCount(value)}${units[scale]}`;
}

/**
 * 美元金额：≥ $1 两位小数；$0.01–$1 最多四位；更小的非零值保留两位有效数字，
 * 绝不显示成 $0.00。低于 $0.000001 时显示为“< $0.000001”。
 */
export function formatUsd(value: number): string {
  if (!Number.isFinite(value)) return "—";
  const abs = Math.abs(value);
  if (abs === 0) return usdCents.format(0);
  if (abs < TINY_USD) return `${value < 0 ? "-" : ""}< ${usdTiny.format(TINY_USD)}`;
  if (abs < 0.01) return usdTiny.format(value);
  return (abs < 1 ? usdSmall : usdCents).format(value);
}

/** 单价（每百万 Token）：最多四位小数，不补零。 */
export function formatRate(value: number | null | undefined, currency: PricingCurrency = "USD"): string {
  return value == null || !Number.isFinite(value) ? "—" : rates[currency].format(value);
}

export function formatPercent(value: number | null | undefined): string {
  return value == null || !Number.isFinite(value) ? "—" : `${oneDecimal.format(value)}%`;
}

export function formatDuration(ms: number | null | undefined, compact = false): string {
  if (ms == null || !Number.isFinite(ms) || ms <= 0) return "—";
  if (compact) {
    if (ms < 1000) return `${Math.round(ms)}ms`;
    return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)}s`;
  }
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
  const seconds = Math.round(ms / 1000);
  return `${Math.floor(seconds / 60)} 分 ${String(seconds % 60).padStart(2, "0")} 秒`;
}

export function formatTps(value: number | null | undefined): string {
  return value == null || !Number.isFinite(value) || value <= 0 ? "—" : `${oneDecimal.format(value)} t/s`;
}

export interface DisplayOptions {
  kind?: "count" | "usd";
  compact?: boolean;
  exactness?: "exact" | "lowerBound";
  unit?: string;
  missingTitle?: string;
}

export function display(value: number | null | undefined, options: DisplayOptions = {}): Display {
  const { kind = "count", compact = false, exactness = "exact", unit = "", missingTitle = "未记录" } = options;
  if (value == null || !Number.isFinite(value)) return { text: "—", prefix: "", missing: true, title: missingTitle };
  const prefix = exactness === "lowerBound" ? "≥" : "";
  const exact = kind === "usd" ? formatUsd(value) : formatCount(value);
  const shown = kind === "count" && compact ? formatTokens(value) : exact;
  const full = `${exact}${unit ? ` ${unit}` : ""}`;
  const title = prefix ? `已知下限：${full}` : full;
  return { text: `${prefix ? `${prefix} ` : ""}${shown}`, prefix, missing: false, title };
}

const pad = (value: number) => String(value).padStart(2, "0");

function localDate(date: Date) {
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())}`;
}

function localClock(date: Date) {
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function formatClock(ms: number) {
  const date = new Date(ms);
  return `${localClock(date)}:${pad(date.getSeconds())}`;
}
export const formatHourMinute = (ms: number) => localClock(new Date(ms));
export function formatMonthDay(ms: number) {
  const date = new Date(ms);
  return `${date.getMonth() + 1}/${date.getDate()}`;
}
export const formatDate = (ms: number) => localDate(new Date(ms));
export function formatDateTime(ms: number) {
  const date = new Date(ms);
  return `${localDate(date)} ${localClock(date)}:${pad(date.getSeconds())}`;
}

/** 时段标题：同一天只写一次日期；整天及以上只写日期。 */
export function formatSpan(start: number, end: number): string {
  const DAY = 86_400_000;
  if (end - start >= DAY - 3_600_000 && new Date(start).getHours() === 0) {
    const last = end - 1;
    return formatDate(start) === formatDate(last) ? formatDate(start) : `${formatMonthDay(start)} – ${formatMonthDay(last)}`;
  }
  const sameDay = formatDate(start) === formatDate(end - 1);
  return `${formatMonthDay(start)} ${formatHourMinute(start)} – ${sameDay ? "" : `${formatMonthDay(end)} `}${formatHourMinute(end)}`;
}
