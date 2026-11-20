import Notice from "../components/Notice";
import SearchField from "../components/SearchField";
import Segmented from "../components/Segmented";
import { Fragment, memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { LoaderCircle, Plus, RotateCw } from "lucide-react";
import * as api from "../api";
import type { AgentInfo } from "../types";
import type { ModelPriceRow, ModelPricesResponse, PricingCurrency, ResolvedPrice, TierRates } from "./types";
import { CNY_PER_USD, formatTokens, formatCount, formatDateTime, formatRate } from "./format";
import { sourceName } from "./metrics";
import ConfirmDialog from "../components/ConfirmDialog";
import HelpTip from "../components/HelpTip";
import ModelLabel from "../components/ModelLabel";
import PriceEditor, { hasEditableCustomPrice, targetModelKey } from "./PriceEditor";
import UiIcon from "../components/UiIcon";

export type PriceStatusFilter = "all" | "unpriced" | "custom" | "reference" | "builtin";

interface Props {
  active: boolean;
  agents: AgentInfo[];
  agent: string;
  revision: string;
  statusFilter: PriceStatusFilter;
  onStatusFilterChange: (status: PriceStatusFilter) => void;
  onPricingChanged: (revision: number) => void;
}

const statusLabels: [PriceStatusFilter, string][] = [
  ["all", "全部"],
  ["unpriced", "未定价"],
  ["custom", "自定义"],
  ["reference", "参考价"],
  ["builtin", "内置"],
];

function priceStatus(price: ResolvedPrice | null): Exclude<PriceStatusFilter, "all"> {
  if (!price) return "unpriced";
  return price.source === "custom" || price.source === "builtin" ? price.source : "reference";
}

function formatPriceUpdateTime(ms: number | null): string {
  return ms ? formatDateTime(ms) : "未获取";
}

const rateFields = ["input", "output", "cacheRead", "cacheWrite"] as const;

function renderRateCell(
  price: ResolvedPrice | null,
  field: typeof rateFields[number],
) {
  if (!price) return "—";
  const base = (price.originalRates ?? price.rates)[field];
  return (
    <span className="price-rate-cell">
      <span>{formatRate(base, price.currency)}</span>
      {price.currency === "CNY" && base != null && <small>{formatRate(price.rates[field], "USD")}</small>}
    </span>
  );
}

function PriceRules({ price }: { price: ResolvedPrice | null }) {
  if (!price) return "—";
  const rules: [string, string][] = [];
  function addRule(label: string, rates: Partial<TierRates>, currency: PricingCurrency = "USD") {
    rules.push([label, rateFields.map(field => {
      const rate = rates[field];
      return rate == null ? "-" : formatRate(rate, currency);
    }).join(" / ")]);
  }
  const long = (price.originalRates ?? price.rates).longContext;
  if (long) addRule(`>${formatTokens(long.threshold)}`, long, price.currency);
  for (const [tier, rates] of Object.entries(price.rates.serviceTiers ?? {})) {
    addRule(tier, rates);
    if (rates.longContext) {
      addRule(`${tier} >${formatTokens(rates.longContext.threshold)}`, rates.longContext);
    }
  }
  const { cacheWrite1h, cacheWrite12h } = price.rates;
  if (cacheWrite1h != null) addRule("cache_write_1h", { cacheWrite: cacheWrite1h });
  if (cacheWrite12h != null) addRule("cache_write_12h", { cacheWrite: cacheWrite12h });
  if (rules.length === 0) return "—";
  return <dl className="price-rules">
    {rules.map(([label, rates]) => <Fragment key={label}><dt>{label}</dt><dd>{rates}</dd></Fragment>)}
  </dl>;
}

function renderSourceCell(price: ResolvedPrice | null) {
  if (!price) {
    return <span className="status-tag" data-tone="warning">未定价</span>;
  }
  if (price.source === "builtin") {
    return <span className="status-tag" data-tone="success">{sourceName(price.source)}</span>;
  }
  if (price.source === "custom") {
    return (
      <div className="price-source-cell">
        <span className="status-tag" data-tone="info">
          自定义（{price.currency}）
        </span>
        {price.overridesReference && (
          <small>
            覆盖 {sourceName(price.overridesReference.source)}（{price.overridesReference.provider}/
            {price.overridesReference.modelId}）
          </small>
        )}
      </div>
    );
  }
  return (
    <div className="price-source-cell">
      <span className="status-tag" data-tone="success">{sourceName(price.source)}</span>
      <small title={`${price.provider ?? ""}/${price.modelId ?? ""}`}>
        {price.provider ? `${price.provider}/${price.modelId ?? ""}` : price.modelId}
      </small>
    </div>
  );
}

const emptyModels: ModelPriceRow[] = [];

export default memo(function ModelPrices({
  active,
  agents,
  agent,
  revision,
  statusFilter,
  onStatusFilterChange,
  onPricingChanged,
}: Props) {
  const [snapshot, setSnapshot] = useState<{ agent: string; data: ModelPricesResponse } | null>(null);
  const data = snapshot?.agent === agent ? snapshot.data : null;
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [actionError, setActionError] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [query, setQuery] = useState("");
  const [editor, setEditor] = useState<{ row: ModelPriceRow | null } | null>(null);
  const [deleting, setDeleting] = useState<ModelPriceRow | null>(null);
  const loadSequence = useRef(0);
  const priceRequest = useRef<string | null>(null);
  const requestKey = `${agent}:${revision}`;

  const agentNames = useMemo(
    () => new Map(agents.map(item => [item.id, item.displayName])),
    [agents],
  );

  const load = useCallback(async () => {
    priceRequest.current = requestKey;
    const seq = ++loadSequence.current;
    setLoading(true);
    setError("");
    try {
      const response = await api.getModelPrices(agent);
      if (seq !== loadSequence.current) return;
      setSnapshot({ agent, data: response });
    } catch (err) {
      if (seq === loadSequence.current) setError(String(err));
    } finally {
      if (seq === loadSequence.current) setLoading(false);
    }
  }, [agent, requestKey]);

  useEffect(() => () => {
    ++loadSequence.current;
    priceRequest.current = null;
  }, []);

  useEffect(() => {
    if (active && priceRequest.current !== requestKey) void load();
  }, [active, load, requestKey]);

  const agentModels = data?.models ?? emptyModels;

  const matchingModels = useMemo(() => {
    const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return agentModels;
    return agentModels.filter(row => {
      const text = [
        row.model,
        row.provider,
        row.price?.source,
        row.price && sourceName(row.price.source),
        row.price?.provider,
        row.price?.modelId,
        row.price?.matchedAs,
        row.price?.overridesReference?.source,
      ]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();
      return words.every(word => text.includes(word));
    });
  }, [agentModels, query]);

  const counts = useMemo(() => {
    const map: Record<PriceStatusFilter, number> = {
      all: matchingModels.length,
      unpriced: 0,
      custom: 0,
      reference: 0,
      builtin: 0,
    };
    for (const row of matchingModels) {
      map[priceStatus(row.price)]++;
    }
    return map;
  }, [matchingModels]);

  const visibleRows = useMemo(
    () =>
      statusFilter === "all"
        ? matchingModels
        : matchingModels.filter(row => priceStatus(row.price) === statusFilter),
    [matchingModels, statusFilter],
  );

  const closeEditor = useCallback(() => setEditor(null), []);
  const priceSaved = (revision: number) => {
    closeEditor();
    setActionError("");
    onPricingChanged(revision);
  };

  const confirmDelete = async () => {
    if (!deleting) return;
    const target = deleting;
    setDeleting(null);
    setActionError("");
    try {
      const deleteKey =
        target.price?.source === "custom" && target.price.modelId
          ? target.price.modelId
          : targetModelKey(target);
      const status = await api.deleteCustomPrice(deleteKey);
      onPricingChanged(status.revision);
    } catch (err) {
      setActionError(`无法删除自定义价格：${String(err)}`);
    }
  };

  const handleRefresh = async () => {
    if (refreshing) return;
    setRefreshing(true);
    setActionError("");
    try {
      const status = await api.refreshReferencePrices();
      const failed = status.sources.filter(source => source.error);
      if (failed.length) {
        setActionError(
          `无法更新 ${failed.map(source => source.name).join("、")} 的参考价。已下载的参考价仍可使用。`,
        );
      }
      if (status.revision > (data?.catalog.revision ?? 0)) {
        onPricingChanged(status.revision);
      } else {
        setSnapshot(current => current && status.revision >= current.data.catalog.revision
          ? { ...current, data: { ...current.data, catalog: status } }
          : current);
      }
    } catch (err) {
      setActionError(`无法更新参考价：${String(err)}`);
    } finally {
      setRefreshing(false);
    }
  };

  return (
    <section className="usage-section prices-panel" aria-label="模型价格" hidden={!active}>
      <div className="prices-heading">
        <div className="prices-title">
          <h2>模型价格</h2>
          <HelpTip label="定价规则">
            报价依次采用自定义价格、内置价格、Models.dev 参考价和 LiteLLM 参考价。
          </HelpTip>
        </div>
        <div className="prices-actions">
          <button type="button" className="button-with-icon" onClick={() => setEditor({ row: null })}>
            <UiIcon icon={Plus} />
            添加自定义价格
          </button>
          <button
            type="button"
            className="button-with-icon"
            disabled={refreshing}
            onClick={() => void handleRefresh()}
          >
            <UiIcon icon={refreshing ? LoaderCircle : RotateCw} className={refreshing ? "usage-spin" : undefined} />
            {refreshing ? "更新中…" : "更新参考价"}
          </button>
        </div>
      </div>

      {data?.catalog && (
        <div className="prices-catalog-bar" role="status">
          <div className="prices-sources">
            {data.catalog.sources.map(source => (
              <div key={source.id} className="prices-source-chip" title={source.url}>
                <span className={`prices-dot ${source.error ? "warn" : source.pricedCount > 0 ? "ok" : ""}`} />
                <strong>{source.name}</strong>
                <span>
                  {source.pricedCount > 0
                    ? `${formatCount(source.pricedCount)} 项 · ${formatPriceUpdateTime(source.fetchedAt)}`
                    : "暂无参考价"}
                </span>
                {source.stale && source.pricedCount > 0 && <em className="prices-stale">已过期</em>}
                {source.error && <em className="prices-src-err" title={source.error}>更新失败</em>}
              </div>
            ))}
          </div>
          <span className="prices-fx-note">汇率 1 USD = {data.catalog.exchangeRateCnyPerUsd} CNY</span>
        </div>
      )}

      {data?.catalog.customError && <Notice tone="error">无法读取自定义价格文件。</Notice>}
      {error && <Notice tone="error" action={{ label: "重试", onClick: () => void load() }}>无法读取模型价格。</Notice>}
      {actionError && <Notice tone="error" onDismiss={() => setActionError("")}>{actionError}</Notice>}

      <div className="prices-toolbar">
        <Segmented label="定价状态筛选" value={statusFilter} onChange={onStatusFilterChange}
          options={statusLabels.map(([id, label]) => [id, <>{label}<span className="prices-filter-count">{formatCount(counts[id])}</span></>])} />
        <SearchField className="prices-search" value={query} onChange={setQuery}
          aria-label="搜索模型价格" placeholder="搜索模型、提供商或来源" />
      </div>

      <div className="prices-table-wrap" tabIndex={0} role="region" aria-label="模型价格表">
        {visibleRows.length > 0 ? (
          <table className="prices-table">
            <colgroup>
              <col style={{ width: 220 }} />
              <col style={{ width: 112 }} />
              <col style={{ width: 98 }} />
              <col style={{ width: 98 }} />
              <col style={{ width: 98 }} />
              <col style={{ width: 98 }} />
              <col style={{ width: 310 }} />
              <col style={{ width: 196 }} />
              <col style={{ width: 132 }} />
            </colgroup>
            <thead>
              <tr>
                <th>模型</th>
                <th className="numeric">请求 / Token</th>
                <th className="numeric">输入 / 1M</th>
                <th className="numeric">输出 / 1M</th>
                <th className="numeric">缓存读取 / 1M</th>
                <th className="numeric">缓存写入 / 1M</th>
                <th title="每百万 Token 的输入 / 输出 / 缓存读取 / 缓存写入单价">额外规则</th>
                <th>报价来源</th>
                <th className="actions-col">操作</th>
              </tr>
            </thead>
            <tbody>
              {visibleRows.map(row => (
                <tr key={`${row.model}\0${row.provider ?? ""}`}>
                  <td className="prices-model-cell">
                    <ModelLabel model={row.model} />
                    <small>
                      {row.customOnly
                        ? "暂无相关请求"
                        : [
                            row.provider ? `提供商 ${row.provider}` : "",
                            row.agents.map(id => agentNames.get(id) ?? id).join(" · "),
                          ]
                            .filter(Boolean)
                            .join(" · ")}
                    </small>
                  </td>
                  <td className="numeric">
                    {row.customOnly ? (
                      "—"
                    ) : (
                      <>
                        <span>{formatCount(row.requests)}</span>
                        <small>{formatTokens(row.totalTokens)} Token</small>
                      </>
                    )}
                  </td>
                  <td className="numeric">{renderRateCell(row.price, "input")}</td>
                  <td className="numeric">{renderRateCell(row.price, "output")}</td>
                  <td className="numeric">{renderRateCell(row.price, "cacheRead")}</td>
                  <td className="numeric">{renderRateCell(row.price, "cacheWrite")}</td>
                  <td><PriceRules price={row.price} /></td>
                  <td>{renderSourceCell(row.price)}</td>
                  <td className="actions-col">
                    <div className="prices-row-actions">
                      <button type="button" className="btn-ghost btn-sm btn-accent" onClick={() => setEditor({ row })}>
                        {hasEditableCustomPrice(row.price) ? "编辑" : "设置价格"}
                      </button>
                      {hasEditableCustomPrice(row.price) && (
                        <button
                          type="button"
                          className="btn-ghost btn-sm danger"
                          onClick={() => setDeleting(row)}
                        >
                          删除
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <div className="request-empty">
            {loading
              ? "正在读取模型价格…"
              : !agentModels.length
                ? "暂无模型记录"
                : statusFilter === "unpriced" && !query.trim()
                  ? "所有模型都已定价"
                  : statusFilter === "custom" && !query.trim()
                    ? "尚未设置自定义价格"
                    : "未找到匹配的模型"}
          </div>
        )}
      </div>

      <div className="prices-footer" role="status">
        <span>
          {visibleRows.length === agentModels.length
            ? `共 ${formatCount(agentModels.length)} 个模型`
            : `显示 ${formatCount(visibleRows.length)} / ${formatCount(agentModels.length)} 个模型`}
        </span>
      </div>

      {editor && (
        <PriceEditor row={editor.row} exchangeRate={data?.catalog.exchangeRateCnyPerUsd ?? CNY_PER_USD}
          onCancel={closeEditor} onSaved={priceSaved} />
      )}

      {deleting && (
        <ConfirmDialog
          title="删除自定义价格"
          message={
            deleting.price?.overridesReference
              ? `删除「${deleting.model}」的自定义价格后，将恢复使用 ${sourceName(deleting.price.overridesReference.source)} 参考价。`
              : `删除「${deleting.model}」的自定义价格后，将重新匹配可用报价。`
          }
          confirmLabel="删除"
          onCancel={() => setDeleting(null)}
          onConfirm={() => void confirmDelete()}
        />
      )}
    </section>
  );
});
