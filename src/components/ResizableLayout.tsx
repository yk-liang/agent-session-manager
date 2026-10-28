import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent, PointerEvent, ReactNode } from "react";
import { DEFAULT_WIDTHS, MIN_WIDTHS, fitPanelWidths, maxPanelWidth, readPanelWidths, resizePanel, savePanelWidths } from "../panelLayout";
import type { Panel, PanelWidths } from "../panelLayout";

interface Props {
  sidebar: ReactNode;
  sessionList?: ReactNode;
  children: ReactNode;
}

interface Drag {
  panel: Panel;
  pointerId: number;
  startX: number;
  startWidths: PanelWidths;
  widths: PanelWidths;
  target: HTMLDivElement;
  changed: boolean;
}

export default function ResizableLayout({ sidebar, sessionList, children }: Props) {
  const container = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const [preferred, setPreferred] = useState(readPanelWidths);
  const [preview, setPreview] = useState<PanelWidths | null>(null);
  const [width, setWidth] = useState(0);
  const [activePanel, setActivePanel] = useState<Panel | null>(null);
  const [keyboardPanel, setKeyboardPanel] = useState<Panel | null>(null);
  const hasSessions = sessionList !== undefined;
  const widths = fitPanelWidths(preview ?? preferred, width, hasSessions);

  const commit = useCallback((next: PanelWidths) => {
    setPreferred(next);
    savePanelWidths(next);
  }, []);

  const finishDrag = useCallback((save: boolean) => {
    const current = drag.current;
    if (!current) return;
    // 先清除状态，防止主动释放捕获又触发 lostpointercapture 重复提交。
    drag.current = null;
    if (current.target.hasPointerCapture(current.pointerId)) current.target.releasePointerCapture(current.pointerId);
    setActivePanel(null);
    setPreview(null);
    if (save && current.changed) commit(current.widths);
  }, [commit]);

  useLayoutEffect(() => {
    const element = container.current;
    if (!element) return;
    const observer = new ResizeObserver(() => {
      finishDrag(false);
      setWidth(element.clientWidth);
    });
    setWidth(element.clientWidth);
    observer.observe(element);
    return () => observer.disconnect();
  }, [finishDrag]);

  useEffect(() => {
    finishDrag(false);
  }, [hasSessions, finishDrag]);

  useEffect(() => {
    if (!activePanel) return;
    document.body.classList.add("resizing-panels");
    const cancel = () => finishDrag(false);
    window.addEventListener("blur", cancel);
    return () => {
      document.body.classList.remove("resizing-panels");
      window.removeEventListener("blur", cancel);
    };
  }, [activePanel, finishDrag]);

  useEffect(() => () => {
    const current = drag.current;
    drag.current = null;
    if (current?.target.hasPointerCapture(current.pointerId)) current.target.releasePointerCapture(current.pointerId);
  }, []);

  const startDrag = (panel: Panel, event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !event.isPrimary || drag.current) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = {
      panel, pointerId: event.pointerId, startX: event.clientX,
      startWidths: widths, widths, target: event.currentTarget, changed: false,
    };
    setKeyboardPanel(null);
    // 保留 Escape/方向键可操作性，但鼠标产生的程序聚焦不作为键盘焦点高亮。
    event.currentTarget.focus({ preventScroll: true });
    setActivePanel(panel);
  };

  const moveDrag = (event: PointerEvent<HTMLDivElement>) => {
    const current = drag.current;
    if (!current || current.pointerId !== event.pointerId) return;
    const requested = current.startWidths[current.panel] + event.clientX - current.startX;
    const next = resizePanel(current.panel, requested, current.startWidths, width, hasSessions);
    current.widths = next;
    current.changed = next[current.panel] !== current.startWidths[current.panel];
    // 用户主动调整时以当前可见宽度为起点，避免窄窗口中的分隔线跳回旧偏好。
    setPreview(next);
  };

  const reset = (panel: Panel) => {
    finishDrag(false);
    commit({ ...preferred, [panel]: DEFAULT_WIDTHS[panel] });
  };

  const resizeWithKey = (panel: Panel, event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") { finishDrag(false); return; }
    if (drag.current) return;
    setKeyboardPanel(panel);
    if (event.key === "Home") { event.preventDefault(); reset(panel); return; }
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const step = (event.shiftKey ? 50 : 10) * (event.key === "ArrowLeft" ? -1 : 1);
    commit(resizePanel(panel, widths[panel] + step, widths, width, hasSessions));
  };

  const divider = (panel: Panel, label: string) => (
    <div
      className={`pane-divider${activePanel === panel ? " active" : ""}${keyboardPanel === panel ? " keyboard-focus" : ""}`}
      role="separator"
      aria-label={label}
      aria-orientation="vertical"
      aria-valuemin={MIN_WIDTHS[panel]}
      aria-valuemax={maxPanelWidth(panel, widths, width, hasSessions)}
      aria-valuenow={widths[panel]}
      aria-valuetext={`${widths[panel]} 像素`}
      tabIndex={0}
      title="拖动调整宽度，双击恢复默认"
      onPointerDown={(event) => startDrag(panel, event)}
      onPointerMove={moveDrag}
      onPointerUp={() => finishDrag(true)}
      onPointerCancel={() => finishDrag(false)}
      onLostPointerCapture={() => finishDrag(true)}
      onDoubleClick={() => reset(panel)}
      onFocus={() => { if (!drag.current) setKeyboardPanel(panel); }}
      onBlur={() => setKeyboardPanel(null)}
      onKeyDown={(event) => resizeWithKey(panel, event)}
    />
  );

  return (
    <div ref={container} className="main" style={{
      "--sidebar-width": `${widths.sidebar}px`,
      "--session-list-width": `${widths.sessions}px`,
    } as CSSProperties}>
      {sidebar}
      {divider("sidebar", "调整侧栏宽度")}
      {sessionList}
      {hasSessions && divider("sessions", "调整会话列表宽度")}
      <div className="detail-pane">{children}</div>
    </div>
  );
}
