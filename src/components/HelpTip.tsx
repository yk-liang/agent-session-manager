import { useEffect, useId, useRef, useState } from "react";
import { CircleHelp } from "lucide-react";
import UiIcon from "./UiIcon";

interface Props {
  label: string;
  children: React.ReactNode;
  above?: boolean;
  align?: "start" | "end";
}

/** 悬浮或聚焦时预览，点击后保留；Escape 和外部点击关闭。 */
export default function HelpTip({
  label,
  children,
  above = false,
  align = "start",
}: Props) {
  const [open, setOpen] = useState(false);
  const pinned = useRef(false);
  const root = useRef<HTMLSpanElement>(null);
  const id = useId();

  useEffect(() => {
    if (!open) return;
    const dismiss = () => {
      pinned.current = false;
      setOpen(false);
    };
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) dismiss();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        dismiss();
      }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape, true);
    };
  }, [open]);

  return (
    <span
      className="help-tip"
      ref={root}
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => {
        if (!pinned.current) setOpen(false);
      }}
    >
      <button
        type="button"
        className="help-button"
        aria-label={`${label}说明`}
        aria-expanded={open}
        aria-describedby={open ? id : undefined}
        onFocus={() => setOpen(true)}
        onBlur={() => {
          if (!pinned.current) setOpen(false);
        }}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          pinned.current = !pinned.current;
          setOpen(pinned.current);
        }}
      >
        <UiIcon icon={CircleHelp} className="help-symbol" />
      </button>
      {open && (
        <span
          id={id}
          role="tooltip"
          className={`help-popover${above ? " above" : ""}${align === "end" ? " align-end" : ""}`}
        >
          {children}
        </span>
      )}
    </span>
  );
}
