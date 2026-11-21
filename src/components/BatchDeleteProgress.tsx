import Modal from "./Modal";

export default function BatchDeleteProgress({ completed, total }: { completed: number; total: number }) {
  return (
    <Modal title="正在删除会话" className="batch-delete-progress">
      <div className="batch-progress-row">
        <progress value={completed} max={total} aria-label="批量删除进度" aria-valuetext={`已处理 ${completed} 个，共 ${total} 个`} />
        <span aria-hidden="true">{completed} / {total}</span>
      </div>
      <p>请等待删除完成，期间无法关闭窗口或退出应用。</p>
    </Modal>
  );
}
