import Notice from "../../components/Notice";
import SearchField from "../../components/SearchField";
import Segmented from "../../components/Segmented";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { LoaderCircle, RotateCw } from "lucide-react";
import * as api from "../../api";
import type { PriceStatusFilter } from "../../uiPreferences";
import type { ModelPriceRow, ModelPricesResponse, ResolvedPrice } from "../types";
import { CNY_PER_USD, formatCount, formatDateTime } from "../format";
import { sourceName } from "../metrics";
import { sortModelPrices } from "./order";
import ConfirmDialog from "../../components/ConfirmDialog";
import { modelIconSource } from "../../components/BrandIcon";
import HelpTip from "../../components/HelpTip";
import ModelPriceTable from "./ModelPriceTable";
import PriceEditor, { customModelKey } from "./PriceEditor";
import UiIcon from "../../components/UiIcon";

interface Props {
  active: boolean;
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

const emptyModels: ModelPriceRow[] = [];

export default memo(function ModelPrices({
  active,
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
  const [editor, setEditor] = useState<ModelPriceRow | null>(null);
  const [deleting, setDeleting] = useState<ModelPriceRow | null>(null);
  const loadSequence = useRef(0);
  const priceRequest = useRef<string | null>(null);
  const requestKey = `${agent}:${revision}`;

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

  const orderedModels = useMemo(() => {
    const groups = new Map<string, ModelPriceRow[]>();
    const unbranded: ModelPriceRow[] = [];
    for (const row of agentModels) {
      const icon = modelIconSource(row.model);
      if (!icon) {
        unbranded.push(row);
        continue;
      }
      const group = groups.get(icon);
      if (group) group.push(row);
      else groups.set(icon, [row]);
    }
    return [...[...groups.values()].flatMap(sortModelPrices), ...unbranded];
  }, [agentModels]);

  const matchingModels = useMemo(() => {
    const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return orderedModels;
    return orderedModels.filter(row => {
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
  }, [orderedModels, query]);

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
    () => statusFilter === "all" ? matchingModels : matchingModels.filter(row => priceStatus(row.price) === statusFilter),
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
      const status = await api.deleteCustomPrice(customModelKey(target));
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

  const emptyLabel = loading ? "正在读取模型价格…"
    : !agentModels.length ? "暂无模型记录"
      : query.trim() ? "未找到匹配的模型"
        : statusFilter === "unpriced" ? "所有模型都已定价"
          : statusFilter === "custom" ? "尚未设置自定义价格" : "未找到匹配的模型";

  return (
    <section className="usage-section prices-panel" aria-label="模型价格" hidden={!active}>
      <div className="prices-heading">
        <div className="prices-title">
          <h2>模型价格</h2>
          <HelpTip label="定价规则">
            <span>报价依次采用自定义价格、内置价格、Models.dev 参考价和 LiteLLM 参考价。</span>
            {data?.catalog && (
              <span className="prices-catalog-info">
                {data.catalog.sources.map(source => (
                  <span key={source.id} className="prices-source-chip" title={source.error ?? source.url}>
                    <span className="prices-source-label">
                      <span className={`prices-dot ${source.error ? "warn" : source.pricedCount > 0 ? "ok" : ""}`} />
                      <strong>{source.name}</strong>
                      <span>{source.pricedCount > 0 ? `${formatCount(source.pricedCount)} 项` : "暂无参考价"}</span>
                    </span>
                    <time className="prices-source-time" dateTime={source.fetchedAt ? new Date(source.fetchedAt).toISOString() : undefined}>
                      {formatPriceUpdateTime(source.fetchedAt)}
                    </time>
                    {source.error ? <em className="prices-src-err">更新失败</em>
                      : source.stale && source.pricedCount > 0 && <em className="prices-stale">已过期</em>}
                  </span>
                ))}
                <span className="prices-fx-note">汇率 1 USD = {data.catalog.exchangeRateCnyPerUsd} CNY</span>
              </span>
            )}
          </HelpTip>
        </div>
        <div className="prices-toolbar">
          <Segmented label="定价状态筛选" value={statusFilter} onChange={onStatusFilterChange}
            options={statusLabels.map(([id, label]) => [id, <>{label}<span className="prices-filter-count">{formatCount(counts[id])}</span></>])} />
          <SearchField className="prices-search" value={query} onChange={setQuery}
            aria-label="搜索模型价格" placeholder="搜索模型、提供商或来源" />
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

      {data?.catalog.customError && <Notice tone="error">无法读取自定义价格文件。</Notice>}
      {error && <Notice tone="error" action={{ label: "重试", onClick: () => void load() }}>无法读取模型价格。</Notice>}
      {actionError && <Notice tone="error" onDismiss={() => setActionError("")}>{actionError}</Notice>}

      <ModelPriceTable active={active} rows={visibleRows} emptyLabel={emptyLabel}
        onEdit={setEditor} onDelete={setDeleting} />

      <div className="prices-footer" role="status">
        <span>
          {visibleRows.length === agentModels.length
            ? `共 ${formatCount(agentModels.length)} 个模型`
            : `显示 ${formatCount(visibleRows.length)} / ${formatCount(agentModels.length)} 个模型`}
        </span>
      </div>

      {editor && (
        <PriceEditor row={editor} exchangeRate={data?.catalog.exchangeRateCnyPerUsd ?? CNY_PER_USD}
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
