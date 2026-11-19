import { useEffect, useId, useState, type FormEvent } from "react";
import { X } from "lucide-react";
import * as api from "../api";
import type { CustomRatesView, ModelPriceRow, PricingCurrency, ReferenceSearchItem, ResolvedPrice } from "./types";
import { formatRate } from "./format";
import { sourceName } from "./metrics";
import UiIcon from "../components/UiIcon";

const RATE_FIELDS = [
  ["input", "输入"],
  ["output", "输出"],
  ["cacheRead", "缓存读取"],
  ["cacheWrite", "缓存写入"],
] as const;

type RateKey = (typeof RATE_FIELDS)[number][0];
type Rates = Pick<CustomRatesView, RateKey>;
type RateDraft = Record<RateKey, string>;

function rateDraft(rates?: Rates | null, scale = 1): RateDraft {
  const value = (key: RateKey) => {
    const amount = rates?.[key];
    return amount == null ? "" : String(scale === 1 ? amount : Number((amount * scale).toPrecision(12)));
  };
  return { input: value("input"), output: value("output"), cacheRead: value("cacheRead"), cacheWrite: value("cacheWrite") };
}

function readRateDraft(draft: RateDraft, prefix = "") {
  const read = (key: RateKey, required = false) => {
    const label = `${prefix}${RATE_FIELDS.find(([id]) => id === key)![1]}单价`;
    const value = draft[key].trim();
    if (!value) {
      if (required) throw new Error(`请输入${label}`);
      return null;
    }
    const amount = Number(value);
    if (!Number.isFinite(amount) || amount < 0) throw new Error(`${label}须为非负数`);
    return amount;
  };
  return { input: read("input", true)!, output: read("output", true)!, cacheRead: read("cacheRead"), cacheWrite: read("cacheWrite") };
}

function PriceRateFields({ currency, draft, disabled, prefix = "", onChange }: {
  currency: PricingCurrency;
  draft: RateDraft;
  disabled: boolean;
  prefix?: string;
  onChange: (draft: RateDraft) => void;
}) {
  return (
    <div className="price-rates-grid">
      {RATE_FIELDS.map(([key, label]) => (
        <label key={key} className="price-field">
          <span>{label} ({currency}/1M){key === "input" || key === "output" ? " *" : ""}</span>
          <input type="number" min="0" step="any" className="control-input" value={draft[key]}
            disabled={disabled} aria-label={`${prefix}${label}单价`}
            onChange={event => onChange({ ...draft, [key]: event.target.value })} />
        </label>
      ))}
    </div>
  );
}

interface EditorState {
  mode: "add" | "edit";
  model: string;
  currency: PricingCurrency;
  rates: RateDraft;
  longContext: { threshold: string; rates: RateDraft } | null;
}

export function hasEditableCustomPrice(price: ResolvedPrice | null): boolean {
  return price?.source === "custom" && !price.matchedAs;
}

export function targetModelKey(row: ModelPriceRow) {
  return row.provider && !row.model.includes("/") ? `${row.provider}/${row.model}` : row.model;
}

function initialDraft(row: ModelPriceRow | null): EditorState {
  if (!row) return { mode: "add", model: "", currency: "USD", rates: rateDraft(), longContext: null };

  const price = row.price;
  const custom = hasEditableCustomPrice(price);
  const rates = (custom ? price?.originalRates : null) ?? price?.rates;
  return {
    mode: "edit",
    model: custom && price?.modelId ? price.modelId : targetModelKey(row),
    currency: custom && price ? price.currency : "USD",
    rates: rateDraft(rates),
    longContext: rates?.longContext ? {
      threshold: String(rates.longContext.threshold), rates: rateDraft(rates.longContext),
    } : null,
  };
}

export default function PriceEditor({ row, exchangeRate, onCancel, onSaved }: {
  row: ModelPriceRow | null;
  exchangeRate: number;
  onCancel: () => void;
  onSaved: (revision: number) => void;
}) {
  const titleId = useId();
  const [editor, setEditor] = useState(() => initialDraft(row));
  const [saving, setSaving] = useState(false);
  const [editorError, setEditorError] = useState("");
  const [referenceQuery, setReferenceQuery] = useState("");
  const [referenceResults, setReferenceResults] = useState<{ query: string; items: ReferenceSearchItem[]; error?: boolean }>();
  const keyword = referenceQuery.trim();
  const search = keyword && referenceResults?.query === keyword ? referenceResults : null;

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented && !saving) onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel, saving]);

  useEffect(() => {
    if (!keyword) return;
    let cancelled = false;
    const timer = window.setTimeout(async () => {
      try {
        const items = await api.searchReferencePrices(keyword, 20);
        if (!cancelled) setReferenceResults({ query: keyword, items });
      } catch {
        if (!cancelled) setReferenceResults({ query: keyword, items: [], error: true });
      }
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [keyword]);

  const applyReference = (item: ReferenceSearchItem) => {
    setEditor(current => {
      const scale = current.currency === "CNY" ? exchangeRate : 1;
      const long = item.rates.longContext;
      return {
        ...current,
        model: current.mode === "add" && !current.model.trim() ? item.modelId : current.model,
        rates: rateDraft(item.rates, scale),
        longContext: long ? { threshold: String(long.threshold), rates: rateDraft(long, scale) } : null,
      };
    });
    setReferenceQuery("");
    setReferenceResults(undefined);
  };

  const updateLongContext = (values: Partial<NonNullable<EditorState["longContext"]>>) => {
    setEditor(current => current.longContext
      ? { ...current, longContext: { ...current.longContext, ...values } }
      : current);
  };

  const handleSave = async (event: FormEvent) => {
    event.preventDefault();
    if (saving) return;
    const model = editor.model.trim();
    if (!model) {
      setEditorError("请输入模型 ID");
      return;
    }

    try {
      const rates = readRateDraft(editor.rates);
      const context = editor.longContext;
      let longContext = null;
      if (context) {
        const threshold = Number(context.threshold);
        if (!Number.isSafeInteger(threshold) || threshold <= 0) {
          throw new Error("请输入大于 0 的整数作为输入阈值");
        }
        longContext = { threshold, ...readRateDraft(context.rates, "长上下文") };
      }
      setEditorError("");
      setSaving(true);
      const status = await api.setCustomPrice({ model, currency: editor.currency, ...rates, longContext });
      onSaved(status.revision);
    } catch (err) {
      setEditorError(err instanceof Error ? err.message : String(err));
      setSaving(false);
    }
  };

  return (
    <div
      className="modal-overlay"
      onClick={() => {
        if (!saving) onCancel();
      }}
    >
      <div
        className="modal price-editor-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onClick={e => e.stopPropagation()}
      >
        <div className="price-editor-header">
          <h3 id={titleId}>
            {editor.mode === "add" ? "添加自定义价格" : "编辑自定义价格"}
          </h3>
          <button
            type="button"
            className="price-editor-close"
            aria-label="关闭"
            disabled={saving}
            onClick={onCancel}
          >
            <UiIcon icon={X} />
          </button>
        </div>
        <form className="price-editor-form" onSubmit={handleSave} noValidate>
          <label className="price-field">
            <span>模型 ID</span>
            <input
              type="text"
              className="control-input"
              value={editor.model}
              disabled={editor.mode === "edit" || saving}
              placeholder="例如 claude-sonnet-4-5"
              autoFocus={editor.mode === "add"}
              onChange={e => setEditor({ ...editor, model: e.target.value })}
            />
          </label>

          <div className="price-ref-lookup">
            <label className="price-field">
              <span>参考价</span>
              <input
                type="search"
                className="control-input"
                value={referenceQuery}
                placeholder="搜索模型或提供商"
                aria-label="搜索参考价"
                autoFocus={editor.mode === "edit"}
                disabled={saving}
                onChange={e => setReferenceQuery(e.target.value)}
              />
            </label>
            {search && search.items.length === 0 && (
              <p className="hint" role="status">{search.error ? "无法读取参考价。" : "未找到匹配的参考价。"}</p>
            )}
            {search && search.items.length > 0 && (
              <div className="price-ref-results" role="group" aria-label="参考价匹配结果">
                {search.items.map(item => (
                  <button
                    key={`${item.source}:${item.provider}:${item.modelId}`}
                    type="button"
                    className="price-ref-option"
                    onClick={() => applyReference(item)}
                  >
                    <span className="price-ref-name">
                      <span className="price-tag ok">{sourceName(item.source)}</span>
                      <code>
                        {item.provider}/{item.modelId}
                      </code>
                    </span>
                    <span className="price-ref-rates">
                      {formatRate(item.rates.input, "USD")} / {formatRate(item.rates.output, "USD")}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>

          <div className="price-currency-row">
            <span>币种</span>
            <div className="usage-segmented" role="group" aria-label="币种">
              {(["USD", "CNY"] as const).map(code => (
                <button
                  key={code}
                  type="button"
                  disabled={saving}
                  aria-pressed={editor.currency === code}
                  onClick={() => setEditor({ ...editor, currency: code })}
                >
                  {code === "USD" ? "USD ($)" : "CNY (¥)"}
                </button>
              ))}
            </div>
            {editor.currency === "CNY" && (
              <small className="price-fx-hint">汇率 1 USD = {exchangeRate} CNY</small>
            )}
          </div>

          <PriceRateFields currency={editor.currency} draft={editor.rates} disabled={saving}
            onChange={rates => setEditor({ ...editor, rates })} />
          <div className="price-context">
            <label className="checkbox-label">
              <input type="checkbox" checked={editor.longContext != null} disabled={saving}
                onChange={event => setEditor({
                  ...editor,
                  longContext: event.target.checked ? { threshold: "", rates: rateDraft() } : null,
                })} />
              长上下文价格
            </label>
            {editor.longContext && <>
              <label className="price-field">
                <span>输入阈值（Token）</span>
                <input type="number" min="1" step="1" className="control-input" disabled={saving}
                  value={editor.longContext.threshold}
                  onChange={event => updateLongContext({ threshold: event.target.value })} />
              </label>
              <PriceRateFields currency={editor.currency} draft={editor.longContext.rates} disabled={saving} prefix="长上下文"
                onChange={rates => updateLongContext({ rates })} />
            </>}
          </div>

          {editorError && (
            <p className="price-editor-error" role="alert">
              {editorError}
            </p>
          )}

          <div className="modal-actions">
            <button type="button" disabled={saving} onClick={onCancel}>
              取消
            </button>
            <button type="submit" className="primary" disabled={saving}>
              {saving ? "保存中…" : "保存"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
