import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ArrowDown, ArrowUp, ListTree, PanelRightClose, PanelRightOpen, X } from "lucide-react";
import UiIcon from "./UiIcon";
import useDismiss from "./useDismiss";

type Entry = { idx: number; text: string };

function outlineSnippet(text: string) {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > 90 ? line.slice(0, 90) + "…" : line;
}

export function useSessionOutline(
  entries: Entry[],
  session: string,
  { current, progress }: { current: number; progress: number },
  onJump: (index: number, options?: { focus?: boolean }) => void,
) {
  const [open, setOpen] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  const id = useId();
  const button = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const currentItem = useRef<HTMLButtonElement>(null);
  const snippets = useMemo(() => entries.map(entry => outlineSnippet(entry.text)), [entries]);

  const active = entries[current]?.idx;
  const previous = entries[current - 1];
  const next = entries[current + 1];

  useEffect(() => { setOpen(false); }, [session]);
  useLayoutEffect(() => {
    if (open) (currentItem.current ?? list.current?.querySelector("button"))?.focus({ preventScroll: true });
  }, [open]);
  useLayoutEffect(() => {
    const item = currentItem.current, container = list.current;
    if (!item || !container?.clientHeight) return;
    const row = item.getBoundingClientRect(), bounds = container.getBoundingClientRect();
    if (row.top < bounds.top || row.bottom > bounds.bottom) {
      container.scrollTop += row.top - bounds.top - (container.clientHeight - row.height) / 2;
    }
  }, [active, collapsed, open]);
  useDismiss(open, [button, panel], reason => {
    setOpen(false);
    if (reason === "escape") button.current?.focus({ preventScroll: true });
  });

  const jump = useCallback((index: number, focus = true) => {
    const overlay = !!button.current?.getClientRects().length;
    if (overlay) setOpen(false);
    onJump(index, { focus: overlay || focus });
  }, [onJump]);
  const items = useMemo(() => entries.map(({ idx }, index) => (
    <li key={idx}>
      <button ref={active === idx ? currentItem : undefined} type="button" className="toc-item btn-ghost"
        title={snippets[index]} aria-label={`第 ${index + 1} 条提问：${snippets[index]}`}
        aria-current={active === idx ? "location" : undefined} data-passed={index < current}
        onClick={() => jump(idx)}>
        <span className="toc-node" aria-hidden="true" />
        <span className="toc-entry">
          <span className="toc-num">#{String(index + 1).padStart(2, "0")}</span>
          <span className="toc-text">{snippets[index]}</span>
        </span>
      </button>
    </li>
  )), [active, current, entries, jump, snippets]);
  const onKeyDown = (event: KeyboardEvent) => {
    if (!entries.length || event.defaultPrevented || event.nativeEvent.isComposing) return;
    if (event.metaKey || event.ctrlKey || event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    const backwards = event.key === "ArrowUp";
    const element = event.target as HTMLElement;
    if (event.altKey && !element.closest("input,textarea,[contenteditable]")) {
      const target = backwards ? previous : next;
      if (target) {
        event.preventDefault();
        jump(target.idx);
      }
    } else if (element.matches(".toc-item")) {
      const item = element.parentElement;
      const sibling = backwards ? item?.previousElementSibling : item?.nextElementSibling;
      event.preventDefault();
      sibling?.querySelector("button")?.focus();
    }
  };

  if (!entries.length) return { trigger: null, navigation: null, onKeyDown };
  const toggleLabel = collapsed ? "展开导览" : "收起导览";
  return {
    onKeyDown,
    trigger: <button ref={button} type="button" className="outline-toggle button-with-icon btn-sm btn-ghost"
      aria-controls={id} aria-expanded={open} onClick={() => setOpen(value => !value)}>
      <UiIcon icon={ListTree} />对话导览
    </button>,
    navigation: <nav className="toc" id={id} ref={panel} aria-label="对话导览" data-open={open} data-collapsed={collapsed}>
      <div className="toc-head">
        <div className="toc-heading">
          <div className="toc-title"><strong>对话导览</strong><span className="status-tag">{entries.length} 条提问</span></div>
          <div className="toc-controls">
            <button type="button" className="btn-icon btn-ghost" title="上一条提问（Alt + ↑）" aria-label="上一条提问" disabled={!previous} onClick={() => previous && jump(previous.idx, false)}><UiIcon icon={ArrowUp} /></button>
            <button type="button" className="btn-icon btn-ghost" title="下一条提问（Alt + ↓）" aria-label="下一条提问" disabled={!next} onClick={() => next && jump(next.idx, false)}><UiIcon icon={ArrowDown} /></button>
          </div>
          <button type="button" className="toc-collapse btn-icon btn-ghost" title={toggleLabel} aria-label={toggleLabel}
            aria-expanded={!collapsed} onClick={() => setCollapsed(value => !value)}>
            <UiIcon icon={collapsed ? PanelRightOpen : PanelRightClose} />
          </button>
          <button type="button" className="toc-close btn-icon btn-ghost" title="关闭导览" aria-label="关闭导览"
            onClick={() => { setOpen(false); button.current?.focus(); }}><UiIcon icon={X} /></button>
        </div>
      </div>
      <div className="toc-progress" role="progressbar" aria-label="阅读进度" aria-valuenow={progress}><span style={{ width: `${progress}%` }} /></div>
      <div className="toc-body" ref={list}>
        <ol className="toc-list">{items}</ol>
      </div>
      <div className="toc-footer"><kbd>Alt</kbd><kbd>↑</kbd><kbd>↓</kbd><span>切换提问</span></div>
    </nav>,
  };
}
