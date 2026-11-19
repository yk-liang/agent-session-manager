import { modelText } from "../models";
import { ModelIcon } from "./BrandIcon";

export default function ModelLabel({ model, badge = false }: { model: string | null; badge?: boolean }) {
  if (!model) return null;
  const label = modelText(model);
  return <span className={`model-label${badge ? " badge" : ""}`} title={label}><ModelIcon id={model} />{label}</span>;
}
