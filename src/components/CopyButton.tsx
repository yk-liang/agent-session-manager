import { useEffect, useRef, useState, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import UiIcon from "./UiIcon";

interface Props {
  value: string;
  label: string;
  title?: string;
  className: string;
  iconClassName?: string;
  children: ReactNode;
  onCopied?: () => void;
  onError: (error: unknown) => void;
}

export default function CopyButton({ value, label, title, className, iconClassName = "", children, onCopied, onError }: Props) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const generation = useRef(0);
  useEffect(() => {
    setCopied(false);
    return () => { ++generation.current; clearTimeout(timer.current); };
  }, [value]);

  const copy = async () => {
    const call = ++generation.current;
    try {
      await writeText(value);
      // 切换会话或卸载后，旧剪贴板请求不能给新的按钮显示成功。
      if (call !== generation.current) return;
      // 只有新一次复制成功才重新计时；等待写入时，上次反馈仍应按时消失。
      clearTimeout(timer.current);
      setCopied(true);
      onCopied?.();
      timer.current = setTimeout(() => setCopied(false), 2000);
    } catch (error) {
      if (call !== generation.current) return;
      clearTimeout(timer.current);
      setCopied(false);
      onError(error);
    }
  };
  return <button className={`copy-button ${className}`} onClick={() => void copy()} aria-label={label} title={copied ? "已复制" : title ?? label} data-copied={copied}>
    {children}<UiIcon icon={copied ? Check : Copy} size={12} className={`copy-feedback-icon ${iconClassName}`} />
    <span className="sr-only" role="status">{copied ? "已复制" : ""}</span>
  </button>;
}
