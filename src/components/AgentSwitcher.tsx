import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { Check, ChevronDown, Layers } from "lucide-react";
import type { AgentInfo } from "../types";
import AgentIcon from "./AgentIcon";
import UiIcon from "./UiIcon";

interface Props {
  agents: AgentInfo[];
  value: string | null;
  allowAll: boolean;
  disabled: boolean;
  onChange: (id: string) => void;
}

export default function AgentSwitcher({ agents, value, allowAll, disabled, onChange }: Props) {
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [open, setOpen] = useState(false);
  const [activeId, setActiveId] = useState<string | null>(null);
  const search = useRef({ text: "", at: 0 });
  const options = [
    ...(allowAll ? [{ id: "all", name: "全部 Agent", disabled: false }] : []),
    ...agents.map(agent => ({ id: agent.id, name: agent.displayName, disabled: !allowAll && !agent.installed })),
  ];
  const enabled = options.filter(option => !option.disabled);
  const unavailable = disabled || enabled.length === 0;
  const selected = options.find(option => option.id === value);
  const active = enabled.find(option => option.id === activeId) ?? enabled.find(option => option.id === value) ?? enabled[0];

  useEffect(() => { setOpen(false); }, [value, allowAll, unavailable]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const close = () => setOpen(false);
    document.addEventListener("pointerdown", outside);
    window.addEventListener("blur", close);
    return () => { document.removeEventListener("pointerdown", outside); window.removeEventListener("blur", close); };
  }, [open]);
  useLayoutEffect(() => {
    if (!open || !active || !list.current) return;
    const option = document.getElementById(`${listId}-${active.id}`);
    if (!option) return;
    // 只滚动选单内部，避免键盘导航把会话或用量页面一起滚走。
    const menu = list.current;
    if (option.offsetTop < menu.scrollTop) menu.scrollTop = option.offsetTop;
    else if (option.offsetTop + option.offsetHeight > menu.scrollTop + menu.clientHeight) {
      menu.scrollTop = option.offsetTop + option.offsetHeight - menu.clientHeight;
    }
  }, [open, active?.id, listId]);

  const show = () => {
    if (unavailable) return;
    search.current = { text: "", at: 0 };
    setActiveId(enabled.find(option => option.id === value)?.id ?? enabled[0].id);
    setOpen(true);
    trigger.current?.focus({ preventScroll: true });
  };
  const choose = (id: string) => {
    if (unavailable || !enabled.some(option => option.id === id)) return;
    setOpen(false);
    trigger.current?.focus({ preventScroll: true });
    if (id !== value) onChange(id);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (unavailable || event.metaKey || event.ctrlKey) return;
    const key = event.key;
    if (key === "Escape" || key === "Tab") {
      if (open) {
        setOpen(false);
        if (key === "Escape") { event.preventDefault(); event.stopPropagation(); }
      }
      return;
    }
    if (key === "Enter" || key === " ") {
      event.preventDefault();
      if (!event.repeat) { if (open && active) choose(active.id); else show(); }
    } else if (["ArrowDown", "ArrowUp", "Home", "End", "PageDown", "PageUp"].includes(key)) {
      event.preventDefault();
      if (event.altKey && key === "ArrowUp" && open && active) { choose(active.id); return; }
      const index = enabled.findIndex(option => option.id === active?.id);
      const next = key === "Home" || key === "PageUp" ? 0 : key === "End" || key === "PageDown" ? enabled.length - 1
        : !open ? enabled.findIndex(option => option.id === value) : index + (key === "ArrowDown" ? 1 : -1);
      setActiveId(enabled[Math.max(0, Math.min(enabled.length - 1, next))].id);
      setOpen(true);
      search.current = { text: "", at: 0 };
    } else if (key.length === 1 && !event.altKey) {
      event.preventDefault();
      const now = Date.now(), text = (open && now - search.current.at < 700 ? search.current.text : "") + key.toLocaleLowerCase();
      search.current = { text, at: now };
      const query = [...text].every(char => char === text[0]) ? text[0] : text;
      const index = enabled.findIndex(option => option.id === (open ? active?.id : value));
      const start = Math.max(0, index + (query.length === 1 ? 1 : 0));
      const match = [...enabled.slice(start), ...enabled.slice(0, start)].find(option => option.name.toLocaleLowerCase().startsWith(query));
      setActiveId(match?.id ?? (open ? active?.id : value) ?? enabled[0].id);
      setOpen(true);
    }
  };

  // macOS 原生 select 会把已选项对齐到按钮，导致靠后的选项把菜单顶出窗口。
  // 选单固定从按钮下方展开；方向键仅预览，明确选择后才触发 Agent 切换和配置保护。
  return <div className="agent-switcher" ref={root} onBlur={event => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOpen(false);
  }}>
    <button ref={trigger} type="button" className="agent-switcher-trigger" role="combobox" aria-label="当前 Agent"
      value={value ?? ""} disabled={unavailable} aria-haspopup="listbox" aria-expanded={open}
      aria-controls={open ? listId : undefined} aria-activedescendant={open && active ? `${listId}-${active.id}` : undefined}
      onKeyDown={onKeyDown} onClick={() => { if (open) setOpen(false); else show(); }}>
      {value && value !== "all" ? <AgentIcon agent={value} /> : <UiIcon icon={Layers} />}
      <span className="agent-switcher-label">{selected?.name ?? "未发现 Agent 数据"}</span>
      <UiIcon icon={ChevronDown} className="agent-switcher-chevron" />
    </button>
    {open && <div ref={list} id={listId} className="agent-switcher-menu" role="listbox" aria-label="Agent" onMouseDown={event => event.preventDefault()}>
      {options.map(option => <button key={option.id} type="button" role="option" id={`${listId}-${option.id}`} value={option.id}
        className="agent-switcher-option" tabIndex={-1} aria-selected={option.id === value} disabled={option.disabled}
        data-active={option.id === active?.id} onPointerMove={() => { if (!option.disabled) setActiveId(option.id); }} onClick={() => choose(option.id)}>
        {option.id === "all" ? <UiIcon icon={Layers} /> : <AgentIcon agent={option.id} />}
        <span className="agent-switcher-label">{option.name}{option.disabled && <small>未发现数据</small>}</span>
        {option.id === value && <UiIcon icon={Check} className="agent-switcher-check" />}
      </button>)}
    </div>}
  </div>;
}
