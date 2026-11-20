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
  children?: ReactNode;
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
      if (call !== generation.current) return;
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
  return <button type="button" className={`copy-button ${className}`} onClick={() => void copy()} aria-label={label} title={copied ? "已复制" : title ?? label} data-copied={copied}>
    {children}<UiIcon icon={copied ? Check : Copy} size={12} className={`copy-feedback-icon ${iconClassName}`} />
    <span className="sr-only" role="status">{copied ? "已复制" : ""}</span>
  </button>;
}
