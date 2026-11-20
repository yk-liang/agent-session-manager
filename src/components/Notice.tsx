import { X } from "lucide-react";
import type { ReactNode } from "react";
import UiIcon from "./UiIcon";

interface Props {
  tone?: "info" | "success" | "warning" | "error";
  className?: string;
  label?: string;
  icon?: ReactNode;
  action?: { label: string; onClick: () => void; disabled?: boolean };
  onDismiss?: () => void;
  children: ReactNode;
}

export default function Notice({ tone = "info", className, label, icon, action, onDismiss, children }: Props) {
  return <div className={className ? `notice ${className}` : "notice"} data-tone={tone}
    role={tone === "error" ? "alert" : "status"} aria-label={label}>
    {icon}
    <div className="notice-content">{children}</div>
    {action && <button type="button" disabled={action.disabled} onClick={action.onClick}>{action.label}</button>}
    {onDismiss && <button type="button" className="btn-icon btn-ghost" aria-label="关闭提示" onClick={onDismiss}><UiIcon icon={X} /></button>}
  </div>;
}
