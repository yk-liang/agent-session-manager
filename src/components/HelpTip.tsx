import { useId, useLayoutEffect, useRef, useState } from "react";
import { CircleHelp } from "lucide-react";
import UiIcon from "./UiIcon";
import useDismiss from "./useDismiss";

interface Props {
  label: string;
  children: React.ReactNode;
  above?: boolean;
}

/** 悬浮或聚焦时预览，点击后保留；Escape 和外部点击关闭。 */
export default function HelpTip({
  label,
  children,
  above = false,
}: Props) {
  const [open, setOpen] = useState(false);
  const pinned = useRef(false);
  const root = useRef<HTMLSpanElement>(null);
  const popover = useRef<HTMLSpanElement>(null);
  const id = useId();

  useLayoutEffect(() => {
    const trigger = root.current;
    const panel = popover.current;
    if (!open || !trigger || !panel) return;
    const place = () => {
      panel.style.removeProperty("--help-offset");
      panel.classList.toggle("above", above);
      const anchor = trigger.getBoundingClientRect();
      const bounds = panel.getBoundingClientRect();
      const roomAbove = anchor.top - 8;
      const roomBelow = window.innerHeight - anchor.bottom - 8;
      const fitsPreferred = bounds.height <= (above ? roomAbove : roomBelow);
      panel.classList.toggle("above", fitsPreferred ? above : roomAbove > roomBelow);
      const correction = Math.max(8, Math.min(bounds.left, window.innerWidth - bounds.width - 8)) - bounds.left;
      panel.style.setProperty("--help-offset", `${correction}px`);
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [above, open]);

  useDismiss(open, [root], () => {
    pinned.current = false;
    setOpen(false);
  });

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
        className="help-button btn-icon btn-ghost"
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
        <UiIcon icon={CircleHelp} />
      </button>
      {open && (
        <span
          ref={popover}
          id={id}
          role="tooltip"
          className="help-popover"
        >
          {children}
        </span>
      )}
    </span>
  );
}
