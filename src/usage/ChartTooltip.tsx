import { Fragment, useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

export interface TooltipRow {
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

/** 触发项在视口中的水平中心和上下边界。 */
export interface TooltipAnchor {
  x: number;
  top: number;
  bottom: number;
}

const MARGIN = 8;
const GAP = 10;

export function ChartTooltip({ id, content, anchor }: { id: string; content: TooltipContent | null; anchor: TooltipAnchor | null }) {
  const box = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = box.current;
    if (!element || !anchor) return;
    const place = () => {
      const { width, height } = element.getBoundingClientRect();
      const left = Math.max(MARGIN, Math.min(anchor.x - width / 2, window.innerWidth - width - MARGIN));
      const above = anchor.top - GAP - height;
      const top = above >= MARGIN ? above : Math.max(MARGIN, Math.min(anchor.bottom + GAP, window.innerHeight - height - MARGIN));
      element.style.transform = `translate(${left}px, ${top}px)`;
    };
    // 首次显示前定位，字体或内容改变尺寸后再校正。
    place();
    const observer = new ResizeObserver(place);
    observer.observe(element);
    return () => observer.disconnect();
  }, [anchor]);
  if (!content || !anchor) return null;
  return createPortal(
    <div ref={box} id={id} role="tooltip" className="chart-tooltip">
      <div className="chart-tooltip-title">{content.title}</div>
      {content.detail && <pre className="chart-tooltip-detail">{content.detail}</pre>}
      {(content.rows.length > 0 || content.total) && (
        <dl className="chart-tooltip-rows">
          {content.rows.map(row => (
            <Fragment key={row.key}>
              <dt className="chart-tooltip-label">
                {row.color && <i style={{ background: row.color }} aria-hidden="true" />}
                <span>{row.label}</span>
              </dt>
              <dd>{row.value}</dd>
            </Fragment>
          ))}
          {content.total && (
            <>
              <dt className="chart-tooltip-total">{content.total.label}</dt>
              <dd className="chart-tooltip-total">{content.total.value}</dd>
            </>
          )}
        </dl>
      )}
      {content.note && <div className="chart-tooltip-note">{content.note}</div>}
    </div>,
    document.body,
  );
}

export function useTooltip() {
  const id = useId();
  const [state, setState] = useState<{ content: TooltipContent; anchor: TooltipAnchor } | null>(null);
  const show = useCallback((content: TooltipContent, anchor: TooltipAnchor) => setState({ content, anchor }), []);
  const hide = useCallback(() => setState(null), []);
  const open = state !== null;
  useEffect(() => {
    if (!open) return;
    const close = () => setState(null);
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    document.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  const tooltip = <ChartTooltip id={id} content={state?.content ?? null} anchor={state?.anchor ?? null} />;
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
