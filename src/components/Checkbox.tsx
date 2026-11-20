import { Check } from "lucide-react";
import UiIcon from "./UiIcon";

interface CheckboxProps {
  checked: boolean;
  disabled?: boolean;
  "aria-label"?: string;
  onChange: (checked: boolean) => void;
}

export function Checkbox({ checked, disabled, onChange, "aria-label": label }: CheckboxProps) {
  return <span className="checkbox-box">
    <input type="checkbox" checked={checked} disabled={disabled} aria-label={label}
      onChange={event => onChange(event.target.checked)} />
    <UiIcon icon={Check} size={12} className="checkbox-check" />
  </span>;
}

export function CheckboxField({ label, title, ...checkbox }: Omit<CheckboxProps, "aria-label"> & {
  label: string;
  title?: string;
}) {
  return <label className="checkbox-label" title={title}><Checkbox {...checkbox} />{label}</label>;
}
