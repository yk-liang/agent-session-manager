import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

interface TooltipRow {
  key: string;
  label: string;
  value: string;
  color?: string;
}

export interface TooltipContent {
  title: string;
  rows: TooltipRow[];
  detail?: string;
  total?: { label: string; value: string };
  note?: string;
}

interface TooltipAnchor {
  x: number;
  top: number;
  bottom: number;
}

const MARGIN = 8;
const GAP = 10;

interface TooltipState {
  content: TooltipContent;
  anchor: TooltipAnchor;
}

function UsageTooltip({ id, content, anchor }: TooltipState & { id: string }) {
  const box = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = box.current;
    if (!element) return;
    const place = () => {
      const { width, height } = element.getBoundingClientRect();
      const left = Math.max(MARGIN, Math.min(anchor.x - width / 2, window.innerWidth - width - MARGIN));
      const above = anchor.top - GAP - height;
      const preferredTop = above >= MARGIN ? above : anchor.bottom + GAP;
      const top = Math.max(MARGIN, Math.min(preferredTop, window.innerHeight - height - MARGIN));
      element.style.transform = `translate(${left}px, ${top}px)`;
    };
    // 首次显示前定位，字体或内容改变尺寸后再校正。
    place();
    const observer = new ResizeObserver(place);
    observer.observe(element);
    return () => observer.disconnect();
  }, [anchor, content]);
  return createPortal(
    <div ref={box} id={id} role="tooltip" className="usage-tooltip">
      <div className="usage-tooltip-title">{content.title}</div>
      {content.detail && <pre className="usage-tooltip-detail">{content.detail}</pre>}
      {(content.rows.length > 0 || content.total) && (
        <dl className="usage-tooltip-rows">
          {content.rows.map(row => (
            <div key={row.key} className="usage-tooltip-row">
              <dt className={`usage-tooltip-label${row.color ? " has-swatch" : ""}`}>
                {row.color && <i style={{ background: row.color }} aria-hidden="true" />}
                {row.label}
              </dt>
              <dd>{row.value}</dd>
            </div>
          ))}
          {content.total && (
            <div className="usage-tooltip-row usage-tooltip-total">
              <dt>{content.total.label}</dt>
              <dd>{content.total.value}</dd>
            </div>
          )}
        </dl>
      )}
      {content.note && <div className="usage-tooltip-note">{content.note}</div>}
    </div>,
    document.body,
  );
}

export function useTooltip() {
  const id = useId();
  const [state, setState] = useState<TooltipState | null>(null);
  const show = useCallback((content: TooltipContent, anchor: TooltipAnchor) => setState({ content, anchor }), []);
  const hide = useCallback(() => setState(null), []);
  const open = state !== null;
  useEffect(() => {
    if (!open) return;
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") hide();
    };
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    document.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
      document.removeEventListener("keydown", escape);
    };
  }, [open, hide]);
  const tooltip = state && <UsageTooltip id={id} {...state} />;
  return { id, open, show, hide, tooltip };
}

export const anchorOf = (element: Element): TooltipAnchor => {
  const rect = element.getBoundingClientRect();
  return { x: rect.left + rect.width / 2, top: rect.top, bottom: rect.bottom };
};

export function TipTrigger({ content, children, className, label }: {
  content: () => TooltipContent;
  children: ReactNode;
  className?: string;
  label?: string;
}) {
  const tip = useTooltip();
  const open = (element: Element) => tip.show(content(), anchorOf(element));
  return (
    <span
      className={`tip-trigger${className ? ` ${className}` : ""}`}
      tabIndex={0}
      aria-label={label}
      aria-describedby={tip.open ? tip.id : undefined}
      onPointerEnter={event => event.pointerType !== "touch" && open(event.currentTarget)}
      onPointerLeave={tip.hide}
      onPointerDown={event => {
        if (event.pointerType === "touch") (tip.open ? tip.hide() : open(event.currentTarget));
      }}
      onFocus={event => event.currentTarget.matches(":focus-visible") && open(event.currentTarget)}
      onBlur={tip.hide}
    >
      {children}
      {tip.tooltip}
    </span>
  );
}
