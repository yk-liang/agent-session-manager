import { DEFAULT_CATALOG_FILTER } from "./catalogCache.ts";
import { DEFAULT_WIDTHS, MIN_WIDTHS, type Panel, type PanelWidths } from "./panelLayout.ts";
import { DEFAULT_VISIBILITY, type Visibility } from "./sessionContent.ts";
import type { CatalogFilter } from "./types";
import type { AnalysisDimension, TrendMetric } from "./usage/metrics";
import type { UsagePeriod } from "./usage/range";
import type { TrendGroup } from "./usage/trend";
import type { RequestOutcome } from "./usage/types";

const STORAGE_KEY = "agent-session-manager.ui-preferences.v1";
const LEGACY_MARKDOWN = "agent-session-manager.markdown.v1";
const LEGACY_PANEL_WIDTHS = "agent-session-manager.panel-widths.v1";

export type UsageTab = "analysis" | "events" | "prices";
export type PriceStatusFilter = "all" | "unpriced" | "custom" | "reference" | "builtin";
export const REQUEST_PAGE_SIZES = [50, 100, 200] as const;
export type RequestPageSize = typeof REQUEST_PAGE_SIZES[number];

export interface RequestSelection {
  model: string | null;
  project: string | null;
  status: RequestOutcome | null;
}

export const defaultRequestSelection: RequestSelection = { model: null, project: null, status: null };

interface UiPreferences {
  // Selecting all Agents in Usage does not replace the Agent used by Sessions and Configuration.
  agents: { local: string | null; usage: string };
  sessionList: CatalogFilter;
  conversation: Visibility & { markdown: boolean };
  panelWidths: PanelWidths;
  usage: { tab: UsageTab; period: UsagePeriod; priceStatus: PriceStatusFilter; requestPageSize: RequestPageSize };
  usageAnalysis: { trendMetric: TrendMetric; trendGroup: TrendGroup; dimension: AnalysisDimension; shareMetric: TrendMetric };
  // Each Agent, including the aggregate "all" view, keeps a separate filter set.
  requestFilters: Record<string, RequestSelection | undefined>;
}

export type PreferenceKey = keyof UiPreferences;
export type PreferenceUpdate<K extends PreferenceKey> = UiPreferences[K] | ((current: UiPreferences[K]) => UiPreferences[K]);

type StorageAccess = Pick<Storage, "getItem" | "setItem">;
const browserStorage: StorageAccess = {
  getItem: key => localStorage.getItem(key),
  setItem: (key, value) => localStorage.setItem(key, value),
};

const record = (value: unknown): Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
const boolean = (value: unknown, fallback: boolean) => typeof value === "boolean" ? value : fallback;
const stringOrNull = (value: unknown) => typeof value === "string" ? value : null;
const agentId = (value: unknown): value is string => typeof value === "string" && /^[a-z][a-z0-9-]*$/.test(value);
const choice = <T>(value: unknown, fallback: T, choices: readonly T[]): T =>
  choices.find(item => item === value) ?? fallback;
const metrics: readonly TrendMetric[] = ["tokens", "requests", "cost"];

function readPreferences(storage: StorageAccess): UiPreferences {
  const read = (key: string) => {
    try { return storage.getItem(key); } catch { return null; }
  };
  const readObject = (key: string) => {
    try { return record(JSON.parse(read(key) ?? "null")); } catch { return {}; }
  };
  const saved = readObject(STORAGE_KEY);
  const agents = record(saved.agents);
  const sessionList = record(saved.sessionList);
  const conversation = record(saved.conversation);
  const panelWidths = record(saved.panelWidths);
  const usage = record(saved.usage);
  const analysis = record(saved.usageAnalysis);

  // A valid new setting (including false) wins. Leave the old keys intact for older builds.
  const legacyWidths = readObject(LEGACY_PANEL_WIDTHS);
  const width = (panel: Panel) => [panelWidths[panel], legacyWidths[panel]].find(
    (value): value is number => typeof value === "number" && Number.isFinite(value) && value >= MIN_WIDTHS[panel],
  ) ?? DEFAULT_WIDTHS[panel];
  return {
    agents: {
      local: agentId(agents.local) && agents.local !== "all" ? agents.local : null,
      usage: agentId(agents.usage) ? agents.usage : "all",
    },
    sessionList: {
      includeEmpty: boolean(sessionList.includeEmpty, DEFAULT_CATALOG_FILTER.includeEmpty),
      includeArchived: boolean(sessionList.includeArchived, DEFAULT_CATALOG_FILTER.includeArchived),
    },
    conversation: {
      tool: boolean(conversation.tool, DEFAULT_VISIBILITY.tool),
      thinking: boolean(conversation.thinking, DEFAULT_VISIBILITY.thinking),
      context: boolean(conversation.context, DEFAULT_VISIBILITY.context),
      markdown: boolean(conversation.markdown, read(LEGACY_MARKDOWN) === "true"),
    },
    panelWidths: { sidebar: width("sidebar"), sessions: width("sessions") },
    usage: {
      tab: choice<UsageTab>(usage.tab, "analysis", ["analysis", "events", "prices"]),
      period: choice<UsagePeriod>(usage.period, "month", ["today", "week", "month", "all"]),
      priceStatus: choice<PriceStatusFilter>(usage.priceStatus, "all", ["all", "unpriced", "custom", "reference", "builtin"]),
      requestPageSize: choice(usage.requestPageSize, 50, REQUEST_PAGE_SIZES),
    },
    usageAnalysis: {
      trendMetric: choice(analysis.trendMetric, "tokens", metrics),
      trendGroup: choice<TrendGroup>(analysis.trendGroup, "category", ["category", "models", "agents"]),
      dimension: choice<AnalysisDimension>(analysis.dimension, "models", ["models", "agents", "projects", "providers"]),
      shareMetric: choice(analysis.shareMetric, "tokens", metrics),
    },
    requestFilters: Object.fromEntries(Object.entries(record(saved.requestFilters)).filter(([agent]) => agentId(agent)).map(([agent, value]) => {
      const entry = record(value);
      return [agent, {
        model: stringOrNull(entry.model),
        // The empty project ID means "project not recorded"; null means no filter.
        project: stringOrNull(entry.project),
        status: choice<RequestOutcome | null>(entry.status, null, ["success", "failed"]),
      }];
    })),
  };
}

export function createUiPreferenceStore(storage: StorageAccess = browserStorage) {
  let current: UiPreferences | undefined;
  const listeners = new Set<() => void>();
  const read = () => current ??= readPreferences(storage);
  return {
    get<K extends PreferenceKey>(key: K) { return read()[key]; },
    set<K extends PreferenceKey>(key: K, update: PreferenceUpdate<K>) {
      const previous = read();
      const value = typeof update === "function" ? update(previous[key]) : update;
      if (Object.is(value, previous[key])) return;
      current = { ...previous, [key]: value };
      try { storage.setItem(STORAGE_KEY, JSON.stringify(current)); } catch {
        // Keep the same in-memory choices across page switches when storage is unavailable.
      }
      listeners.forEach(listener => listener());
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
  };
}
