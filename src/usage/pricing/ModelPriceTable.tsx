import { useState } from "react";
import Modal from "../../components/Modal";
import ModelLabel from "../../components/ModelLabel";
import { qualifiedModelId } from "../../models";
import { formatRate, formatTokens } from "../format";
import { sourceName } from "../metrics";
import type { ModelPriceRow, PricingCurrency, QuoteRates, ResolvedPrice, TokenRates } from "../types";
import { hasEditableCustomPrice } from "./PriceEditor";

const rateColumns = [
  ["input", "输入"],
  ["output", "输出"],
  ["cacheRead", "缓存读取"],
  ["cacheWrite", "缓存写入"],
] as const;

interface PriceDisplay {
  currency: PricingCurrency;
  rates: QuoteRates;
  originalRates?: TokenRates | null;
}

type ShortTtl = "5m" | "30m";

function shortCacheTtl(rates: QuoteRates): ShortTtl | undefined {
  if (rates.cacheWrite12h != null) return "30m";
  if (rates.cacheWrite1h != null) return "5m";
}

function PriceRateHeadings({ unit = "" }: { unit?: string }) {
  return <>{rateColumns.map(([field, label]) => <th key={field} scope="col" className="numeric">{label}{unit}</th>)}</>;
}

function PriceRate({ value, currency = "USD", usd }: {
  value: number | null | undefined;
  currency?: PricingCurrency;
  usd?: number | null;
}) {
  return (
    <span className="price-rate-cell">
      <span>{formatRate(value, currency)}</span>
      {currency === "CNY" && usd != null && <small>({formatRate(usd)})</small>}
    </span>
  );
}

function CacheWritePrices({ price, shortTtl }: { price: PriceDisplay; shortTtl?: ShortTtl }) {
  const { cacheWrite: usd, cacheWrite1h, cacheWrite12h } = price.rates;
  const value = (price.originalRates ?? price.rates).cacheWrite;
  const ttl = shortCacheTtl(price.rates) ?? shortTtl;
  if (!ttl || (value == null && cacheWrite1h == null && cacheWrite12h == null)) {
    return <PriceRate value={value} currency={price.currency} usd={usd} />;
  }
  return <span className="price-cache-rates">
    {value != null && <span className="price-inline">
      <span className="status-tag">{ttl}</span>
      <PriceRate value={value} currency={price.currency} usd={usd} />
    </span>}
    {cacheWrite1h != null && <span className="price-inline">
      <span className="status-tag">1h</span><PriceRate value={cacheWrite1h} />
    </span>}
    {cacheWrite12h != null && <span className="price-inline">
      <span className="status-tag">12h</span><PriceRate value={cacheWrite12h} />
    </span>}
  </span>;
}

function PriceRateCells({ price, shortTtl }: { price: PriceDisplay | null; shortTtl?: ShortTtl }) {
  const rates = price?.originalRates ?? price?.rates;
  return <>
    {rateColumns.map(([field]) => <td key={field} className="numeric">
      {field === "cacheWrite"
        ? price ? <CacheWritePrices price={price} shortTtl={shortTtl} /> : "—"
        : <PriceRate value={rates?.[field]} currency={price?.currency} usd={price?.rates[field]} />}
    </td>)}
  </>;
}

interface PriceVariant extends PriceDisplay {
  tier: string;
  context: string;
}

function priceVariants(price: ResolvedPrice): PriceVariant[] {
  const variants: PriceVariant[] = [];
  const long = price.rates.longContext;
  if (long) variants.push({
    tier: "默认", context: `>${formatTokens(long.threshold)}`, rates: long,
    originalRates: price.originalRates?.longContext, currency: price.currency,
  });
  for (const [name, rates] of Object.entries(price.rates.serviceTiers ?? {})) {
    const tier = name.charAt(0).toUpperCase() + name.slice(1);
    const threshold = rates.longContext?.threshold ?? long?.threshold;
    variants.push({ tier, context: threshold == null ? "—" : `≤${formatTokens(threshold)}`, rates, currency: "USD" });
    const tierLong = rates.longContext;
    if (tierLong) {
      variants.push({ tier, context: `>${formatTokens(tierLong.threshold)}`, rates: tierLong, currency: "USD" });
    }
  }
  return variants;
}

function PriceVariantsDialog({ model, price, variants, onClose }: {
  model: string;
  price: ResolvedPrice;
  variants: PriceVariant[];
  onClose: () => void;
}) {
  const { longContext } = price.rates;
  const shortTtl = shortCacheTtl(price.rates);
  const rows: PriceVariant[] = shortTtl
    ? [{ ...price, tier: "默认", context: longContext ? `≤${formatTokens(longContext.threshold)}` : "—" }, ...variants]
    : variants;
  return <Modal title={<>{model} <span className="price-variants-subtitle">价格变体</span></>}
    className="price-variants-modal" onClose={onClose}>
    <div className="price-variants-table-wrap">
      <table className="prices-table price-variants-table">
        <caption>每百万 Token 单价</caption>
        <thead>
          <tr>
            <th scope="col">Tier</th>
            <th scope="col">上下文</th>
            <PriceRateHeadings />
          </tr>
        </thead>
        <tbody>
          {rows.map(variant => <tr key={`${variant.tier}:${variant.context}`}>
            <td>{variant.tier}</td>
            <td>{variant.context}</td>
            <PriceRateCells price={variant} shortTtl={shortTtl} />
          </tr>)}
        </tbody>
      </table>
    </div>
  </Modal>;
}

function PriceSource({ price }: { price: ResolvedPrice | null }) {
  if (!price) {
    return <span className="status-tag" data-tone="warning">未定价</span>;
  }
  if (price.source === "builtin") {
    return <span className="status-tag" data-tone="success">{sourceName(price.source)}</span>;
  }
  if (price.source === "custom") {
    const ref = price.overridesReference;
    const override = ref && `覆盖 ${sourceName(ref.source)}（${qualifiedModelId(ref.modelId, ref.provider)}）`;
    return (
      <div className="price-source-cell price-inline">
        <span className="status-tag" data-tone="info">
          自定义（{price.currency}）
        </span>
        {override && <small title={override}>{override}</small>}
      </div>
    );
  }
  const referenceId = price.modelId && qualifiedModelId(price.modelId, price.provider);
  return (
    <div className="price-source-cell price-inline">
      <span className="status-tag" data-tone="success">{sourceName(price.source)}</span>
      {referenceId && <span className="price-source-id" title={referenceId}>{referenceId}</span>}
    </div>
  );
}

interface Props {
  active: boolean;
  rows: ModelPriceRow[];
  emptyLabel: string;
  onEdit: (row: ModelPriceRow) => void;
  onDelete: (row: ModelPriceRow) => void;
}

export default function ModelPriceTable({ active, rows, emptyLabel, onEdit, onDelete }: Props) {
  const [variantsDialog, setVariantsDialog] = useState<{
    model: string; price: ResolvedPrice; variants: PriceVariant[];
  } | null>(null);

  return (
    <>
      <div className="prices-table-wrap" tabIndex={0} role="region" aria-label="模型价格表">
        {rows.length > 0 ? (
          <table className="prices-table">
            <thead>
              <tr>
                <th>模型</th>
                <PriceRateHeadings unit=" / 1M" />
                <th className="price-variants-col">价格变体</th>
                <th>报价来源</th>
                <th className="actions-col">操作</th>
              </tr>
            </thead>
            <tbody>
              {rows.map(row => {
                const { price } = row;
                const variants = price ? priceVariants(price) : [];
                const editable = hasEditableCustomPrice(price);
                return <tr key={`${row.model}\0${row.provider ?? ""}`}>
                  <td className="prices-model-cell"><ModelLabel model={row.model} /></td>
                  <PriceRateCells price={price} />
                  <td className="price-variants-col">{price && variants.length > 0 ? <span className="price-variants-action">
                    <span className="status-tag">{variants.length} 条</span>
                    <button type="button" className="btn-ghost btn-sm btn-accent" aria-haspopup="dialog"
                      onClick={() => setVariantsDialog({ model: row.model, price, variants })}>
                      价格变体
                    </button>
                  </span> : "—"}</td>
                  <td><PriceSource price={price} /></td>
                  <td className="actions-col">
                    <div className="prices-row-actions">
                      <button type="button" className="btn-ghost btn-sm btn-accent" onClick={() => onEdit(row)}>
                        {editable ? "编辑" : "设置价格"}
                      </button>
                      {editable && (
                        <button
                          type="button"
                          className="btn-ghost btn-sm danger"
                          onClick={() => onDelete(row)}
                        >
                          删除
                        </button>
                      )}
                    </div>
                  </td>
                </tr>;
              })}
            </tbody>
          </table>
        ) : (
          <div className="request-empty">{emptyLabel}</div>
        )}
      </div>
      {active && variantsDialog && <PriceVariantsDialog {...variantsDialog} onClose={() => setVariantsDialog(null)} />}
    </>
  );
}
