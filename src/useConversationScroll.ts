import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from "react";
import type { Virtualizer } from "@tanstack/react-virtual";

type Prompt = { idx: number };
type JumpOptions = { focus?: boolean; match?: boolean };

function promptAt(prompts: Prompt[], index: number) {
  let low = 0, high = prompts.length;
  while (low < high) {
    const mid = (low + high) >>> 1;
    if (prompts[mid].idx <= index) low = mid + 1;
    else high = mid;
  }
  return Math.max(0, low - 1);
}

export function useConversationScroll(
  session: string,
  prompts: Prompt[],
  visible: number[],
  virtual: Virtualizer<HTMLDivElement, Element>,
  body: RefObject<HTMLDivElement | null>,
  content: RefObject<HTMLDivElement | null>,
) {
  const [position, setPosition] = useState({ current: 0, progress: 0 });
  const frame = useRef(0);
  const jumpFrame = useRef(0);
  const anchor = useRef<{ index: number; offset?: number } | null>(null);

  const track = useCallback(() => {
    const viewport = body.current, list = content.current;
    if (!viewport || !list || !prompts.length) return;
    const { scrollTop, scrollHeight, clientHeight } = viewport;
    const max = Math.max(0, scrollHeight - clientHeight);
    const readingLine = scrollTop + clientHeight * (max > 0 && scrollTop >= max - 4 ? 1 : 0.35);
    const listStart = list.getBoundingClientRect().top - viewport.getBoundingClientRect().top + scrollTop;
    const row = virtual.getVirtualItemForOffset(Math.max(0, readingLine - listStart));
    if (anchor.current?.offset !== undefined && Math.abs(scrollTop - anchor.current.offset) > 1) {
      anchor.current = null;
    }
    const current = promptAt(prompts, anchor.current?.index ?? (typeof row?.key === "number" ? row.key : 0));
    const progress = max > 0 ? Math.round(Math.max(0, Math.min(1, scrollTop / max)) * 100) : 100;
    setPosition(previous => previous.current === current && previous.progress === progress ? previous : { current, progress });
  }, [body, content, prompts, virtual]);

  const cancelJump = useCallback(() => {
    cancelAnimationFrame(jumpFrame.current);
    virtual.cancelScroll();
    anchor.current = null;
  }, [virtual]);

  useLayoutEffect(() => {
    body.current?.scrollTo(0, 0);
    return cancelJump;
  }, [body, cancelJump, session]);

  useLayoutEffect(() => {
    const viewport = body.current, list = content.current;
    if (!viewport || !list) return;
    const schedule = () => {
      if (frame.current) return;
      frame.current = requestAnimationFrame(() => { frame.current = 0; track(); });
    };
    const release = () => { cancelJump(); schedule(); };
    const onPointerDown = (event: PointerEvent) => { if (event.target === viewport) release(); };
    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.altKey && ["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End"].includes(event.key)) release();
    };
    const observer = new ResizeObserver(schedule);
    observer.observe(viewport);
    observer.observe(list);
    viewport.addEventListener("scroll", schedule, { passive: true });
    viewport.addEventListener("wheel", release, { passive: true });
    viewport.addEventListener("touchstart", release, { passive: true });
    viewport.addEventListener("pointerdown", onPointerDown);
    viewport.addEventListener("keydown", onKeyDown);
    track();
    return () => {
      cancelAnimationFrame(frame.current);
      frame.current = 0;
      observer.disconnect();
      viewport.removeEventListener("scroll", schedule);
      viewport.removeEventListener("wheel", release);
      viewport.removeEventListener("touchstart", release);
      viewport.removeEventListener("pointerdown", onPointerDown);
      viewport.removeEventListener("keydown", onKeyDown);
    };
  }, [body, content, cancelJump, track]);

  const jumpTo = useCallback((index: number, { focus = false, match = false }: JumpOptions = {}) => {
    const viewport = body.current;
    const rowIndex = visible.indexOf(index);
    if (!viewport || rowIndex < 0) return;
    cancelJump();
    if (!match) anchor.current = { index };
    virtual.scrollToIndex(rowIndex, { align: "start" });
    track();

    // 等待虚拟行与搜索高亮挂载，再在正文滚动区内精确定位。
    let frames = 0;
    const settle = () => {
      const row = viewport.querySelector<HTMLElement>(`#ev-${index}`);
      if ((!row && frames < 20) || frames < 2 || (match && row?.querySelector("[data-markdown-pending]") && frames < 120)) {
        frames++;
        jumpFrame.current = requestAnimationFrame(settle);
        return;
      }
      virtual.cancelScroll();
      if (row) {
        const mark = match ? row.querySelector("mark.search-highlight") : null;
        const rect = (mark ?? row).getBoundingClientRect();
        const inset = mark ? (viewport.clientHeight - rect.height) / 2 : parseFloat(getComputedStyle(viewport).paddingTop);
        viewport.scrollTop += rect.top - viewport.getBoundingClientRect().top - inset;
        if (focus) row.focus({ preventScroll: true });
        row.classList.remove("flash");
        void row.offsetWidth;
        row.classList.add("flash");
        if (anchor.current) anchor.current.offset = viewport.scrollTop;
      } else {
        anchor.current = null;
      }
      track();
    };
    jumpFrame.current = requestAnimationFrame(settle);
  }, [body, cancelJump, track, virtual, visible]);

  return { position, jumpTo };
}
