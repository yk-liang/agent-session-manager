import type { KeyboardEvent, ReactNode } from "react";

interface Props<T extends string> {
  label: string;
  options: readonly (readonly [T, ReactNode])[];
  value: T;
  onChange: (value: T) => void;
  compact?: boolean;
  tabs?: boolean;
  disabled?: boolean;
}

export default function Segmented<T extends string>({ label, options, value, onChange, compact, tabs, disabled }: Props<T>) {
  const moveTab = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const last = options.length - 1;
    const next = event.key === "ArrowRight" ? (index + 1) % options.length
      : event.key === "ArrowLeft" ? (index + last) % options.length
      : event.key === "Home" ? 0 : event.key === "End" ? last : undefined;
    if (next === undefined) return;
    event.preventDefault();
    onChange(options[next][0]);
    event.currentTarget.parentElement?.querySelectorAll("button")[next]?.focus();
  };
  return <div className={compact ? "segmented compact" : "segmented"} role={tabs ? "tablist" : "group"} aria-label={label}>
    {options.map(([id, content], index) => <button key={id} type="button" className="btn-ghost"
      role={tabs ? "tab" : undefined} aria-selected={tabs ? value === id : undefined}
      aria-pressed={tabs ? undefined : value === id} tabIndex={tabs && value !== id ? -1 : undefined}
      onKeyDown={tabs ? event => moveTab(event, index) : undefined}
      disabled={disabled} onClick={() => onChange(id)}>{content}</button>)}
  </div>;
}
