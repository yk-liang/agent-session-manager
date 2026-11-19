import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Check, ChevronDown } from "lucide-react";
import UiIcon from "./UiIcon";

interface Option {
  id: string;
  name: string;
  icon?: ReactNode;
}

interface Props {
  label: string;
  options: Option[];
  value: string | null;
  onChange: (id: string) => void;
  disabled?: boolean;
  placeholder?: string;
  searchLabel?: string;
  className?: string;
}

function positionMenu(button: HTMLButtonElement, menu: HTMLDivElement) {
  const anchor = button.getBoundingClientRect();
  const gap = 6, margin = 8;
  const below = window.innerHeight - anchor.bottom - gap - margin;
  const above = anchor.top - gap - margin;
  menu.style.setProperty("--dropdown-anchor-width", `${anchor.width}px`);
  menu.style.removeProperty("--dropdown-available-height");
  const opensBelow = menu.offsetHeight <= below || below >= above;
  menu.style.setProperty("--dropdown-available-height", `${Math.max(0, opensBelow ? below : above)}px`);
  const rightLimit = window.innerWidth - menu.offsetWidth - margin;
  const left = anchor.left <= rightLimit ? anchor.left : Math.min(anchor.right - menu.offsetWidth, rightLimit);
  menu.style.left = `${Math.max(margin, left)}px`;
  menu.style.top = `${opensBelow ? anchor.bottom + gap : anchor.top - gap - menu.offsetHeight}px`;
}

export default function Dropdown({ label, options, value, onChange, disabled = false,
  placeholder, searchLabel, className }: Props) {
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeId, setActiveId] = useState<string | null>(null);
  const typeahead = useRef({ text: "", at: 0 });
  const unavailable = disabled || options.length === 0;
  const selected = options.find(option => option.id === value);
  const filter = open ? query.trim().toLocaleLowerCase() : "";
  const visible = filter ? options.filter(option => option.name.toLocaleLowerCase().includes(filter) ||
    option.id.toLocaleLowerCase().includes(filter)) : options;
  const active = visible.find(option => option.id === activeId) ?? (filter ? visible[0] : selected ?? visible[0]);
  const optionId = (id: string) => `${listId}-${encodeURIComponent(id)}`;
  const activeDescendant = open && active ? optionId(active.id) : undefined;

  useEffect(() => { setOpen(false); }, [value, unavailable]);
  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    const outside = (event: Event) => {
      if (!root.current?.contains(event.target as Node)) close();
    };
    // WebKit 点击按钮时可能先失焦；同一触发按钮的点击交给 onClick 切换。
    document.addEventListener("pointerdown", outside);
    document.addEventListener("focusin", outside);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("focusin", outside);
      window.removeEventListener("blur", close);
    };
  }, [open]);
  useLayoutEffect(() => {
    const button = trigger.current, popup = menu.current;
    if (!button || !popup) return;
    const position = () => positionMenu(button, popup);
    const onScroll = ({ target }: Event) => {
      if (target instanceof HTMLElement) {
        if (!target.contains(button)) return;
        const anchor = button.getBoundingClientRect(), bounds = target.getBoundingClientRect();
        if (anchor.bottom <= bounds.top || anchor.top >= bounds.bottom) {
          setOpen(false);
          return;
        }
      }
      position();
    };
    position();
    window.addEventListener("resize", position);
    document.addEventListener("scroll", onScroll, true);
    const observer = new ResizeObserver(position);
    observer.observe(button);
    return () => {
      window.removeEventListener("resize", position);
      document.removeEventListener("scroll", onScroll, true);
      observer.disconnect();
    };
  }, [open, visible.length]);
  useLayoutEffect(() => {
    if (open && searchLabel) search.current?.focus({ preventScroll: true });
  }, [open, searchLabel]);
  useLayoutEffect(() => {
    const items = list.current;
    const option = activeDescendant && document.getElementById(activeDescendant);
    if (!option || !items) return;
    // 只滚动选单内部，避免键盘导航把会话或用量页面一起滚走。
    if (option.offsetTop < items.scrollTop) items.scrollTop = option.offsetTop;
    else if (option.offsetTop + option.offsetHeight > items.scrollTop + items.clientHeight) {
      items.scrollTop = option.offsetTop + option.offsetHeight - items.clientHeight;
    }
  }, [activeDescendant, visible.length]);

  const show = (id = selected?.id ?? options[0]?.id) => {
    setQuery("");
    typeahead.current = { text: "", at: 0 };
    setActiveId(id ?? null);
    setOpen(true);
    trigger.current?.focus({ preventScroll: true });
  };
  const close = () => {
    trigger.current?.focus({ preventScroll: true });
    setOpen(false);
  };
  const choose = (id: string) => {
    close();
    if (id !== value) onChange(id);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (unavailable || event.nativeEvent.isComposing || event.metaKey || event.ctrlKey) return;
    const key = event.key;
    const typing = event.currentTarget === search.current;
    if (key === "Escape" || key === "Tab") {
      if (open) {
        close();
        if (key === "Escape") { event.preventDefault(); event.stopPropagation(); }
      }
      return;
    }
    if (key === "Enter" || (key === " " && !typing)) {
      event.preventDefault();
      if (!event.repeat) {
        if (!open) show();
        else if (active) choose(active.id);
      }
    } else if (["ArrowDown", "ArrowUp", "PageDown", "PageUp"].includes(key) ||
      (!typing && (key === "Home" || key === "End"))) {
      event.preventDefault();
      const first = key === "Home" || key === "PageUp", last = key === "End" || key === "PageDown";
      if (!open) {
        show(first ? options[0].id : last ? options[options.length - 1].id : undefined);
        return;
      }
      if (event.altKey && key === "ArrowUp" && active) { choose(active.id); return; }
      const index = visible.findIndex(option => option.id === active?.id);
      const next = first ? 0 : last ? visible.length - 1 : index + (key === "ArrowDown" ? 1 : -1);
      setActiveId(visible[Math.max(0, Math.min(visible.length - 1, next))]?.id ?? null);
      typeahead.current = { text: "", at: 0 };
    } else if (!searchLabel && key.length === 1 && !event.altKey) {
      event.preventDefault();
      const now = Date.now();
      const text = (open && now - typeahead.current.at < 700 ? typeahead.current.text : "") + key.toLocaleLowerCase();
      const prefix = [...text].every(char => char === text[0]) ? text[0] : text;
      const index = options.findIndex(option => option.id === (open ? active?.id : value));
      const start = Math.max(0, index + (prefix.length === 1 ? 1 : 0));
      const match = [...options.slice(start), ...options.slice(0, start)].find(option => option.name.toLocaleLowerCase().startsWith(prefix));
      if (open) setActiveId(match?.id ?? active?.id ?? null); else show(match?.id);
      typeahead.current = { text, at: now };
    }
  };

  return <div className={className ? `dropdown ${className}` : "dropdown"} ref={root}>
    <button ref={trigger} type="button" className="dropdown-trigger" role="combobox" aria-label={label}
      value={value ?? ""} disabled={unavailable} aria-haspopup="listbox" aria-expanded={open}
      aria-controls={open ? listId : undefined} aria-activedescendant={searchLabel ? undefined : activeDescendant}
      onKeyDown={onKeyDown} onMouseDown={event => { if (open) event.preventDefault(); }}
      onClick={() => { if (open) close(); else show(); }}>
      {selected?.icon}
      <span className="dropdown-label" title={selected?.name}>{selected?.name ?? placeholder}</span>
      <UiIcon icon={ChevronDown} className="dropdown-chevron" />
    </button>
    {open && <div ref={menu} className="dropdown-menu">
      {searchLabel && <input ref={search} className="dropdown-search" type="search" role="combobox" tabIndex={-1}
        aria-label={searchLabel} placeholder={searchLabel} autoComplete="off" value={query}
        aria-autocomplete="list" aria-expanded="true" aria-controls={listId} aria-activedescendant={activeDescendant}
        onKeyDown={onKeyDown} onChange={event => { setQuery(event.target.value); setActiveId(null); }} />}
      <div ref={list} id={listId} className="dropdown-options" role="listbox" aria-label={label}>
        {visible.map(option => <button key={option.id} type="button" role="option" id={optionId(option.id)} value={option.id}
          className="dropdown-option" tabIndex={-1} aria-selected={option.id === value} data-active={option.id === active?.id}
          onMouseDown={event => event.preventDefault()} onPointerMove={() => setActiveId(option.id)} onClick={() => choose(option.id)}>
          {option.icon}
          <span className="dropdown-label" title={option.name}>{option.name}</span>
          {option.id === value && <UiIcon icon={Check} className="dropdown-check" />}
        </button>)}
      </div>
      {!visible.length && <p className="dropdown-empty" role="status">未找到匹配项</p>}
    </div>}
  </div>;
}
