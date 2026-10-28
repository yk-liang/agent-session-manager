import { Check } from "lucide-react";
import UiIcon from "./UiIcon";

/** 保留原生复选框的语义和键盘操作，外观由 CSS/Lucide 绘制，避免 macOS 失焦时改色。 */
export default function ViewToggle({ label, checked, onChange, title }: {
  label: string;
  title?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return <label className="toggle" title={title}>
    <span className="view-toggle-box">
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
      <UiIcon icon={Check} size={12} className="view-toggle-check" />
    </span>
    {label}
  </label>;
}
