import type { ModelPriceRow } from "../types";
import { bareModelId } from "../../models.ts";

const modelOrder = new Intl.Collator("en", { numeric: true, sensitivity: "base" });
const variantOrder = new Map(
  ["pro", "astra", "opus", "", "sol", "sonnet", "codex", "terra", "mini", "flash", "luna", "nano", "haiku", "lite"]
    .map((variant, index) => [variant, index]),
);

function modelParts(model: string) {
  const name = bareModelId(model).toLowerCase();
  const snapshot = name.match(/-(20\d{2})-?(\d{2})-?(\d{2})$/);
  const base = snapshot ? name.slice(0, snapshot.index) : name;
  const normalized = base.replace(/^claude-(\d+(?:[-.]\d+)?)-(opus|sonnet|haiku)(?=-|$)/, "claude-$2-$1");
  const claude = /^claude-(opus|sonnet|haiku)-(\d+)(?:[-.](\d+))?(?:-(.*))?$/.exec(normalized);
  const standard = claude ? null : /^([a-z]+(?:-[a-z]+)*?)[-_]?(\d+(?:\.\d+)*)(.*)$/.exec(normalized);
  const variant = claude ? claude[1] : (standard ? standard[3].replace(/^[-_.:]/, "") : "");
  return {
    family: claude ? "claude" : (standard ? standard[1] : base),
    version: claude ? [claude[2], claude[3]].filter(Boolean).join(".") : (standard ? standard[2] : ""),
    variant,
    variantRank: variantOrder.get(variant.split("-")[0]) ?? variantOrder.size,
    suffix: claude ? claude[4] ?? "" : "",
    snapshot: snapshot?.slice(1).join("") ?? "",
  };
}

export function sortModelPrices(rows: ModelPriceRow[]): ModelPriceRow[] {
  return rows.map(row => {
    const own = modelParts(row.model);
    // 无版本号的别名（如 codex-auto-review）紧跟对应的定价模型。
    const matched = row.price?.matchedAs && row.price.modelId && !own.version ? modelParts(row.price.modelId) : null;
    const isAlias = !!matched?.version;
    return { row, parts: isAlias && matched ? matched : own, isAlias, qualified: row.model.includes("/") };
  }).sort((a, b) =>
    modelOrder.compare(a.parts.family, b.parts.family)
    || modelOrder.compare(b.parts.version, a.parts.version)
    || a.parts.variantRank - b.parts.variantRank
    || modelOrder.compare(a.parts.variant, b.parts.variant)
    || modelOrder.compare(a.parts.suffix, b.parts.suffix)
    || Number(a.isAlias) - Number(b.isAlias)
    || Number(!!a.parts.snapshot) - Number(!!b.parts.snapshot)
    || modelOrder.compare(b.parts.snapshot, a.parts.snapshot)
    || Number(a.qualified) - Number(b.qualified)
    || modelOrder.compare(a.row.model, b.row.model)
    || modelOrder.compare(a.row.provider ?? "", b.row.provider ?? ""),
  ).map(entry => entry.row);
}
