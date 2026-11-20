export interface UsageTokens {
  inputRecorded?: boolean;
  outputRecorded?: boolean;
  inputIncomplete?: boolean;
  outputIncomplete?: boolean;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  /** 包含在 cacheWriteTokens 中的 1 小时缓存写入。 */
  cacheWrite1hTokens?: number;
  reasoningTokens: number;
  unclassifiedTokens: number;
  totalTokens: number;
  cacheReadRecorded?: boolean;
  cacheWriteRecorded?: boolean;
  reasoningRecorded?: boolean;
}
export interface UsageRow extends UsageTokens {
  date: string;
  model: string;
  partial: boolean;
}
export interface UsageRequestMetrics {
  // 累计差额不能还原请求次数；此时 requestCount 只是可确认的下限。
  requestCount: number;
  requestCountIncomplete: boolean;
  successCount?: number;
  failedCount?: number;
  // 按模型价格估算的美元费用。null 表示无可用估算。
  costUsd: number | null;
  customCostUsd?: number | null;
  referenceCostUsd?: number | null;
  costUnknownCount?: number;
  unpricedCount?: number;
  /** 计量字段缺失或只有下限，无法估算费用的请求数。 */
  costUsageIncompleteCount?: number;
  /** 服务档位或计费分项没有对应价格的请求数。 */
  costSpecialCount?: number;
  costIncomplete: boolean;
  cacheReadIncomplete: boolean;
  cacheInputTokens: number | null;
  cacheInputReadTokens: number;
  cacheInputWriteTokens: number;
  cacheExcludedCount: number;
}
export interface UsageSummaryRow extends UsageRow, UsageRequestMetrics {
  agent: string;
  bucketStart: number | null;
  recordCount: number;
}
export interface DimensionSummaryRow extends UsageTokens, UsageRequestMetrics {
  agent: string;
  model: string;
  key: string;
  recordCount: number;
  partial: boolean;
}
export interface UsageSummary {
  rows: UsageSummaryRow[];
  projects?: DimensionSummaryRow[];
  providers?: DimensionSummaryRow[];
  // 与 rows 来自同一快照；空目录也必须完成同步后才能确认用量为 0。
  syncedAgents: string[];
  /** 各来源缺少发生时间的记录数；有时间范围的查询不包含它们。 */
  untimed?: Record<string, number>;
  revision: string;
  pricingRevision?: number;
}
export interface UsageSummaryQuery { from?: number; to?: number; bounds: number[] }
export interface UsageSyncState {
  sequence: number;
  revision: number;
  updatedAt: number | null;
  syncing: string[];
  recollecting: string[];
  progress: UsageSyncProgress | null;
  failedAgents: string[];
  unavailable: boolean;
}
export interface UsageSyncProgress {
  agent: string;
  phase: "waiting" | "discovering" | "reading" | "indexing" | "complete";
  checkedFiles: number;
  totalFiles: number | null;
}

export interface RequestMetadata {
  requestId: string | null;
  provider: string | null;
  project: string | null;
  absoluteProjectPath?: string;
  client: string | null;
  kind: string | null;
  subagentId?: string;
  parentSessionId?: string;
  effort: string | null;
  tier: string | null;
  version: string | null;
  source: string | null;
  stop: string | null;
  httpStatus: number | null;
  error?: { kind?: string; message?: string };
  startedAt: number | null;
  finishedAt: number | null;
  timing: "recorded" | "observed" | "codex_boundary" | null;
  latencyMs: number | null;
  ttftMs: number | null;
  ttftScope: "request" | "turn" | null;
  scope: "request" | "usage_delta" | "error" | null;
}
export type PricingCurrency = "USD" | "CNY";

export interface LongContextRates {
  threshold: number;
  input: number;
  output: number;
  cacheRead?: number | null;
  cacheWrite?: number | null;
}

export interface TierRates {
  input: number;
  output: number;
  cacheRead?: number | null;
  cacheWrite?: number | null;
  longContext?: LongContextRates | null;
}

export interface PriceRates extends TierRates {
  /** 1 小时缓存写入单价；只用于日志明确记录为 1 小时 TTL 的写入。 */
  cacheWrite1h?: number | null;
  cacheWrite12h?: number | null;
  /** priority / flex / batch 档位单价；只用于记录为该档位的请求。 */
  serviceTiers?: Record<string, TierRates>;
}

export interface CustomRatesView {
  input: number;
  output: number;
  cacheRead?: number | null;
  cacheWrite?: number | null;
  longContext?: LongContextRates | null;
}

export interface ReferenceSummary {
  source: string;
  provider: string;
  modelId: string;
  rates: PriceRates;
  matchedAs?: string | null;
}

type PriceSource = "builtin" | "custom" | "models.dev" | "litellm";

export interface ResolvedPrice {
  source: PriceSource;
  provider?: string | null;
  modelId?: string | null;
  matchedAs?: string | null;
  currency: PricingCurrency;
  originalRates?: CustomRatesView | null;
  rates: PriceRates;
  overridesReference?: ReferenceSummary | null;
}

export interface EventCost {
  totalUsd: number;
  source: PriceSource | "no_usage";
  currency: PricingCurrency;
  inputUsd: number;
  cacheReadUsd: number;
  cacheWriteUsd: number;
  outputUsd: number;
  inputRate: number;
  outputRate: number;
  cacheReadRate: number;
  cacheWriteRate: number;
  cacheWrite1hUsd?: number | null;
  cacheWrite1hRate?: number | null;
  longContext: boolean;
  serviceTier?: string | null;
  multiplier?: number | null;
}

export type PricingIssue =
  | "usage_incomplete"
  | "unclassified"
  | "cache_ambiguous"
  | "unpriced"
  | "unsupported_tier"
  | "unsupported_component";

export interface PricingSourceStatus {
  id: string;
  name: string;
  url: string;
  fetchedAt: number | null;
  modelCount: number;
  pricedCount: number;
  stale: boolean;
  error?: string | null;
}

export interface PricingCatalogStatus {
  revision: number;
  sources: PricingSourceStatus[];
  customCount: number;
  customError?: string | null;
  exchangeRateCnyPerUsd: number;
}

export interface ModelPriceRow {
  model: string;
  provider?: string | null;
  agents: string[];
  providers: string[];
  requests: number;
  totalTokens: number;
  price: ResolvedPrice | null;
  customOnly: boolean;
}

export interface ModelPricesResponse {
  models: ModelPriceRow[];
  catalog: PricingCatalogStatus;
}

export interface ReferenceSearchItem {
  source: string;
  provider: string;
  modelId: string;
  rates: PriceRates;
}

export interface CustomPriceInput {
  model: string;
  currency: PricingCurrency;
  input: number;
  output: number;
  cacheRead: number | null;
  cacheWrite: number | null;
  longContext?: LongContextRates | null;
}

export type RequestOutcome = "success" | "failed";

export interface RequestEvent extends Omit<UsageTokens, "totalTokens"> {
  id: string;
  agent: string;
  time: number;
  session: string;
  model: string;
  status: RequestOutcome;
  partial: boolean;
  request: RequestMetadata;
  outputTps: number | null;
  cost?: EventCost | null;
  pricingIssue?: PricingIssue | null;
}
export type StatusCounts = Record<RequestOutcome, number>;
/** 筛选结果在本地时间对齐的时段内的请求数，覆盖全部分页。 */
export interface RequestTimelineData {
  bounds: number[];
  daysPerBucket?: number;
  success: number[];
  failed: number[];
  /** 缺少发生时间的记录，按时间倒序排在最后。 */
  untimed: StatusCounts;
}
export interface EventQuery {
  agent?: string;
  model?: string;
  project?: string;
  includeFilters?: boolean;
  status?: RequestOutcome;
  from?: number;
  to?: number;
  page?: number;
  limit?: number;
  // 自动同步时定位当前可见请求；主动跳页只使用 page。
  anchor?: string;
}
export interface RequestFilters {
  models: Record<string, number>;
  projects: Record<string, number>;
  results: Record<string, number>;
}
export interface EventPage {
  events: RequestEvent[];
  total: number;
  /** 当前筛选范围内是否有缓存写入，覆盖所有分页。 */
  hasCacheWrites: boolean;
  hasServiceTiers: boolean;
  page: number;
  pageSize: number;
  revision: string;
  pricingRevision?: number;
  timeline?: RequestTimelineData;
  filters?: RequestFilters;
}
