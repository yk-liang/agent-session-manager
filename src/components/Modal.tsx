import { useEffect, useEffectEvent, useId, useLayoutEffect, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import UiIcon from "./UiIcon";

interface Props {
  title: string;
  className?: string;
  closeLabel?: string;
  busy?: boolean;
  onClose: () => void;
  children: ReactNode;
}

export default function Modal({ title, className, closeLabel = "关闭", busy = false, onClose, children }: Props) {
  const titleId = useId();
  const dialog = useRef<HTMLDivElement>(null);
  const opener = useRef(document.activeElement instanceof HTMLElement ? document.activeElement : null);
  const dismiss = useEffectEvent(() => { if (!busy) onClose(); });

  useLayoutEffect(() => {
    const element = dialog.current;
    const previous = opener.current;
    if (element && !element.contains(document.activeElement)) element.focus({ preventScroll: true });
    return () => queueMicrotask(() => {
      if (!element?.isConnected && document.activeElement === document.body) previous?.focus({ preventScroll: true });
    });
  }, []);
  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented && !event.isComposing) dismiss();
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, []);

  return createPortal(
    <div className="modal-overlay" onClick={event => {
      if (event.target === event.currentTarget && !busy) onClose();
    }}>
      <div ref={dialog} className={className ? `modal ${className}` : "modal"} role="dialog"
        aria-modal="true" aria-labelledby={titleId} tabIndex={-1} onKeyDown={event => {
          if (event.key !== "Tab" || event.defaultPrevented) return;
          const controls = [...event.currentTarget.querySelectorAll<HTMLElement>(
            "button, input, select, textarea, a[href], summary, [tabindex]",
          )].filter(element => !element.matches(":disabled, [tabindex='-1']") && element.getClientRects().length);
          const first = controls[0], last = controls[controls.length - 1];
          if (!first || (event.shiftKey ? event.target === first || event.target === dialog.current : event.target === last)) {
            event.preventDefault();
            (event.shiftKey ? last : first)?.focus();
          }
        }}>
        <header className="modal-header">
          <h2 className="modal-title" id={titleId}>{title}</h2>
          <button type="button" className="btn-icon btn-ghost" aria-label={closeLabel} disabled={busy} onClick={onClose}>
            <UiIcon icon={X} />
          </button>
        </header>
        {children}
      </div>
    </div>,
    document.body,
  );
}
