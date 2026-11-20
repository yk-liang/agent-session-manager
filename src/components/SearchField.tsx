import { useRef, type ComponentPropsWithoutRef, type RefObject } from "react";
import { Search, X } from "lucide-react";
import UiIcon from "./UiIcon";

interface Props extends Omit<ComponentPropsWithoutRef<"input">, "type" | "value" | "onChange"> {
  ref?: RefObject<HTMLInputElement | null>;
  value: string;
  onChange: (value: string) => void;
}

export default function SearchField({ ref, className, value, onChange, disabled, tabIndex, ...inputProps }: Props) {
  const fallbackRef = useRef<HTMLInputElement>(null);
  const input = ref ?? fallbackRef;
  return <div className={className ? `search-field ${className}` : "search-field"}>
    <UiIcon icon={Search} size={16} className="search-icon" />
    <input {...inputProps} ref={input} type="search" className="control-input" value={value}
      onChange={event => onChange(event.target.value)} disabled={disabled} tabIndex={tabIndex} />
    {value && <button type="button" className="search-clear btn-icon btn-ghost" aria-label="清除搜索"
      title="清除搜索" disabled={disabled} tabIndex={tabIndex} onClick={() => {
        onChange("");
        input.current?.focus({ preventScroll: true });
      }}><UiIcon icon={X} /></button>}
  </div>;
}
