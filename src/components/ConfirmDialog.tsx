import { useEffect, useId } from "react";

interface Props {
  title: string;
  message: React.ReactNode;
  confirmLabel?: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export default function ConfirmDialog({
  title,
  message,
  confirmLabel = "确认",
  onConfirm,
  onCancel,
}: Props) {
  const titleId = useId();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby={titleId} onClick={(e) => e.stopPropagation()}>
        <div className="modal-title" id={titleId}>{title}</div>
        <div className="modal-body">{message}</div>
        <div className="modal-actions">
          <button onClick={onCancel} autoFocus>取消</button>
          <button className="danger solid" onClick={onConfirm}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
