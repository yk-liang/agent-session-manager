import { ModelIcon } from "./BrandIcon";

export default function ModelLabel({ model, badge = false }: { model: string | null; badge?: boolean }) {
  if (!model) return null;
  return <span className={badge ? "model-label status-tag" : "model-label"} data-tone={badge ? "info" : undefined} title={model}>
    <ModelIcon id={model} />{model}
  </span>;
}
