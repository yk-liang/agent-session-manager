import { useContext, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { DetailState } from "./DetailState";
import UiIcon from "./UiIcon";

/** 展开状态按 `id` 保存在会话阅读状态里，虚拟列表重新挂载后保持不变。 */
export default function Collapsible({ id, className, defaultOpen = false, head, children }: {
  id: string;
  className: string;
  defaultOpen?: boolean;
  head: ReactNode;
  children: ReactNode;
}) {
  const { expanded } = useContext(DetailState);
  const [open, setOpen] = useState(() => expanded.get(id) ?? defaultOpen);
  const toggle = () => {
    expanded.set(id, !open);
    setOpen(!open);
  };
  return <div className={`collapsible ${className}`}>
    <button type="button" className="collapsible-head" aria-expanded={open} onClick={toggle}>
      <UiIcon icon={open ? ChevronDown : ChevronRight} className="caret" />
      {head}
    </button>
    {open && <div className="collapsible-body">{children}</div>}
  </div>;
}
