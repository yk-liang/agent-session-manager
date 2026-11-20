import Modal from "./Modal";

interface Props {
  title: string;
  message: React.ReactNode;
  confirmLabel?: string;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export default function ConfirmDialog({
  title,
  message,
  confirmLabel = "确认",
  busy = false,
  onConfirm,
  onCancel,
}: Props) {
  return (
    <Modal title={title} busy={busy} onClose={onCancel}>
        <div className="modal-body">{message}</div>
        <div className="modal-actions">
          <button type="button" onClick={onCancel} autoFocus disabled={busy}>取消</button>
          <button type="button" className="danger solid" onClick={onConfirm} disabled={busy}>
            {confirmLabel}
          </button>
        </div>
    </Modal>
  );
}
