import { ModelIcon } from "./BrandIcon";

export default function ModelLabel({ model, badge = false }: { model: string | null; badge?: boolean }) {
  if (!model) return null;
  return <span className={`model-label${badge ? " badge" : ""}`} title={model}><ModelIcon id={model} />{model}</span>;
}
