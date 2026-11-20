import { useState } from "react";
import { clearOrphanRequests } from "../api";
import { CheckboxField } from "../components/Checkbox";
import Dropdown from "../components/Dropdown";
import Modal from "../components/Modal";
import Notice from "../components/Notice";
import type { AgentInfo } from "../types";

const maxDays = 0xffff_ffff;
const ranges = [
  { id: "all", name: "不限时间" },
  ...[7, 30, 90, 180, 365].map(days => ({ id: String(days), name: `${days} 天前` })),
  { id: "custom", name: "自定义" },
];

interface Props {
  agents: AgentInfo[];
  agent: string;
  onComplete: (removed: number) => void;
  onCancel: () => void;
}

export default function OrphanCleanupDialog({ agents, agent, onComplete, onCancel }: Props) {
  const [selected, setSelected] = useState(() => agents.filter(a => agent === "all" || a.id === agent).map(a => a.id));
  const [range, setRange] = useState("all");
  const [customDays, setCustomDays] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const days = range === "all" ? null : Number(range === "custom" ? customDays : range);
  const validRange = days === null || (Number.isInteger(days) && days > 0 && days <= maxDays);
  const canClean = !busy && selected.length > 0 && validRange;

  const clean = async () => {
    if (!canClean) return;
    setBusy(true);
    setError("");
    try {
      onComplete(await clearOrphanRequests(selected, days));
    } catch (error) {
      setError(String(error));
      setBusy(false);
    }
  };

  return (
    <Modal title="清理孤儿记录" busy={busy} onClose={onCancel}>
      <form onSubmit={event => { event.preventDefault(); void clean(); }}>
        <div className="modal-body orphan-cleanup-fields">
          <p>永久删除关联会话已不存在的请求记录。</p>
          <div className="orphan-cleanup-range">
            <span>时间范围</span>
            <div className="orphan-cleanup-range-controls">
              <Dropdown label="时间范围" options={ranges} value={range} disabled={busy} onChange={setRange} />
              {range === "custom" && (
                <label className="orphan-cleanup-custom">
                  <input className="control-input" type="number" inputMode="numeric" aria-label="自定义天数"
                    min={1} max={maxDays} step={1} autoFocus disabled={busy} value={customDays}
                    onChange={event => setCustomDays(event.target.value)} />
                  <span>天前</span>
                </label>
              )}
            </div>
          </div>
          <fieldset disabled={busy}>
            <legend>Agent</legend>
            <div className="orphan-cleanup-agents">
              {agents.map(a => (
                <CheckboxField key={a.id} label={a.displayName} checked={selected.includes(a.id)}
                  onChange={checked => setSelected(ids => checked ? [...ids, a.id] : ids.filter(id => id !== a.id))} />
              ))}
            </div>
          </fieldset>
          {error && <Notice tone="error">{error}</Notice>}
        </div>
        <div className="modal-actions">
          <button type="button" autoFocus disabled={busy} onClick={onCancel}>取消</button>
          <button type="submit" className="danger solid" disabled={!canClean}>{busy ? "清理中…" : "清理"}</button>
        </div>
      </form>
    </Modal>
  );
}
