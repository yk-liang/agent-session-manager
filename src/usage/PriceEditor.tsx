import { useEffect, useId, useState, type FormEvent } from "react";
import * as api from "../api";
import type { CustomRatesView, ModelPriceRow, PricingCurrency, ReferenceSearchItem, ResolvedPrice } from "./types";
import { formatRate } from "./format";
import { sourceName } from "./metrics";
import { CheckboxField } from "../components/Checkbox";
import Modal from "../components/Modal";
import Notice from "../components/Notice";
import SearchField from "../components/SearchField";
import Segmented from "../components/Segmented";

const RATE_FIELDS = [
  ["input", "输入"],
  ["output", "输出"],
  ["cacheRead", "缓存读取"],
  ["cacheWrite", "缓存写入"],
] as const;

type RateKey = (typeof RATE_FIELDS)[number][0];
type Rates = Pick<CustomRatesView, RateKey>;
type RateDraft = Record<RateKey, string>;

function rateDraft(rates?: Rates | RateDraft | null, scale = 1): RateDraft {
  const value = (key: RateKey) => {
    const amount = rates?.[key];
    if (amount == null || amount === "") return "";
    if (scale === 1) return String(amount);
    const converted = Number(amount) * scale;
    if (!Number.isFinite(converted)) throw new Error("单价超出可换算范围。");
    return String(Number(converted.toPrecision(15)));
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
  const referenceId = useId();
  const [editor, setEditor] = useState(() => initialDraft(row));
  const [saving, setSaving] = useState(false);
  const [editorError, setEditorError] = useState("");
  const [referenceQuery, setReferenceQuery] = useState("");
  const [referenceResults, setReferenceResults] = useState<{ query: string; items: ReferenceSearchItem[]; error?: boolean }>();
  const keyword = referenceQuery.trim();
  const search = keyword && referenceResults?.query === keyword ? referenceResults : null;

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
    const scale = editor.currency === "CNY" ? exchangeRate : 1;
    const long = item.rates.longContext;
    try {
      setEditor({
        ...editor,
        model: editor.mode === "add" && !editor.model.trim() ? item.modelId : editor.model,
        rates: rateDraft(item.rates, scale),
        longContext: long ? { threshold: String(long.threshold), rates: rateDraft(long, scale) } : null,
      });
      setEditorError("");
      setReferenceQuery("");
      setReferenceResults(undefined);
    } catch (error) {
      setEditorError((error as Error).message);
    }
  };

  const changeCurrency = (currency: PricingCurrency) => {
    if (editor.currency === currency) return;
    const scale = currency === "CNY" ? exchangeRate : 1 / exchangeRate;
    const context = editor.longContext;
    try {
      setEditor({
        ...editor,
        currency,
        rates: rateDraft(editor.rates, scale),
        longContext: context ? { ...context, rates: rateDraft(context.rates, scale) } : null,
      });
      setEditorError("");
    } catch (error) {
      setEditorError((error as Error).message);
    }
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
    <Modal title={editor.mode === "add" ? "添加自定义价格" : "编辑自定义价格"}
      className="price-editor-modal" busy={saving} onClose={onCancel}>
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
          <div className="price-field">
            <label htmlFor={referenceId}>参考价</label>
            <SearchField id={referenceId} value={referenceQuery} onChange={setReferenceQuery}
              placeholder="搜索模型或提供商" aria-label="搜索参考价" autoFocus={editor.mode === "edit"} disabled={saving} />
          </div>
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
                  disabled={saving}
                  onClick={() => applyReference(item)}
                >
                  <span className="price-ref-name">
                    <span className="status-tag" data-tone="success">{sourceName(item.source)}</span>
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
          <Segmented compact label="币种" options={[["USD", "USD ($)"], ["CNY", "CNY (¥)"]]}
            value={editor.currency} onChange={changeCurrency} disabled={saving} />
          <small className="price-fx-hint">切换时按 1 USD = {exchangeRate} CNY 换算单价</small>
        </div>

        <PriceRateFields currency={editor.currency} draft={editor.rates} disabled={saving}
          onChange={rates => setEditor({ ...editor, rates })} />
        <div className="price-context">
          <CheckboxField label="长上下文价格" checked={editor.longContext != null} disabled={saving}
            onChange={checked => setEditor({
              ...editor,
              longContext: checked ? { threshold: "", rates: rateDraft() } : null,
            })} />
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

        {editorError && <Notice tone="error">{editorError}</Notice>}

        <div className="modal-actions">
          <button type="button" disabled={saving} onClick={onCancel}>
            取消
          </button>
          <button type="submit" className="primary" disabled={saving}>
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
