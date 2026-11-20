import { useEffect, useId, useRef, useState } from "react";
import { CircleAlert, LoaderCircle } from "lucide-react";
import type { AgentInfo } from "../types";
import type { UsageSyncProgress, UsageSyncState } from "./types";
import { useDelayedBusy } from "../useDelayedBusy";
import { formatClock, formatCount, formatDateTime } from "./format";
import UiIcon from "../components/UiIcon";
import useDismiss from "../components/useDismiss";

const PHASES: Record<UsageSyncProgress["phase"], string> = {
  waiting: "等待同步",
  discovering: "查找用量记录",
  reading: "读取用量记录",
  indexing: "整理请求",
  complete: "已完成",
};

export function formatSyncProgress(progress: UsageSyncProgress) {
  const files = progress.totalFiles == null ? ""
    : ` ${formatCount(progress.checkedFiles)} / ${formatCount(progress.totalFiles)} 个文件`;
  return PHASES[progress.phase] + files;
}

export default function UsageSyncStatus({ agents, synced, status, unavailable, onRetry, onRecollect }: {
  agents: AgentInfo[];
  synced: ReadonlySet<string>;
  status: UsageSyncState | null;
  unavailable: boolean;
  onRetry: () => void;
  onRecollect: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [pulseAt, setPulseAt] = useState<number | null>(null);
  const lastUpdate = useRef<number | null | undefined>(undefined);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  useDismiss(open, [root], reason => {
    setOpen(false);
    if (reason === "escape") trigger.current?.focus({ preventScroll: true });
  });

  const busy = Boolean(status?.syncing.length || status?.recollecting.length);
  const readingHistory = status?.syncing.some(id => !synced.has(id)) ?? false;
  const showBusy = useDelayedBusy(!!status?.progress || readingHistory, 250, 0);
  const updatedAt = status?.updatedAt;
  const updatedTimeLabel = updatedAt == null ? null : formatDateTime(updatedAt);
  const syncFailed = unavailable || !!status?.failedAgents.length;
  const state = syncFailed ? "error" : showBusy || !status ? "busy" : "live";
  useEffect(() => {
    if (updatedAt === undefined) return;
    const previous = lastUpdate.current;
    if (state !== "live") {
      if (previous === undefined) lastUpdate.current = updatedAt;
      setPulseAt(null);
      return;
    }
    lastUpdate.current = updatedAt;
    const changed = previous !== undefined && updatedAt !== null && (previous === null || updatedAt > previous);
    if (changed && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      setPulseAt(updatedAt);
    }
  }, [updatedAt, state]);
  const label = unavailable ? "同步暂不可用"
    : syncFailed ? "同步失败"
    : showBusy ? (readingHistory ? "同步历史用量…" : "正在更新…")
    : !status ? "正在准备同步…"
    : updatedAt == null ? "自动更新" : `${formatClock(updatedAt)} 更新`;
  const agentSyncState = (id: string, recollecting: boolean) => {
    const progress = status?.progress;
    if (progress?.agent === id) return { text: formatSyncProgress(progress), tone: "busy" };
    if (recollecting) return { text: "等待重新采集", tone: "busy" };
    if (status?.failedAgents.includes(id)) return { text: "读取失败，等待重试", tone: "error" };
    if (status?.syncing.includes(id)) return { text: "等待同步", tone: "busy" };
    return synced.has(id) ? { text: "已同步", tone: "ok" } : { text: "等待首次同步", tone: "busy" };
  };
  return (
    <div className="usage-sync" ref={root}>
      <button
        ref={trigger}
        type="button"
        className="usage-sync-trigger"
        data-state={state}
        aria-expanded={open}
        aria-controls={panelId}
        aria-label={`同步状态，${label}`}
        title={state === "live" && updatedTimeLabel ? `最近更新 ${updatedTimeLabel}` : undefined}
        onClick={() => setOpen(!open)}
      >
        {state === "live" ? (
          <span className="usage-sync-dot" aria-hidden="true">
            {pulseAt !== null && (
              <span key={pulseAt} className="usage-sync-pulse" onAnimationEnd={() => setPulseAt(null)} />
            )}
          </span>
        ) :
          <UiIcon icon={state === "error" ? CircleAlert : LoaderCircle} className={state === "busy" ? "usage-spin" : undefined} />}
        <span>{label}</span>
      </button>
      {open && (
        <div id={panelId} className="usage-sync-panel" role="dialog" aria-label="用量同步状态">
          <div className="usage-sync-heading">
            <strong>用量同步</strong>
            <button type="button" disabled={busy} onClick={onRetry}>{syncFailed ? "重试同步" : "立即检查"}</button>
          </div>
          {updatedAt != null && <p>最近更新 <time dateTime={new Date(updatedAt).toISOString()}>{updatedTimeLabel}</time></p>}
          <ul className="usage-sync-agents">
            {agents.map(agent => {
              const recollecting = status?.recollecting.includes(agent.id) ?? false;
              const state = agentSyncState(agent.id, recollecting);
              return (
                <li key={agent.id}>
                  <div className="usage-sync-agent-info">
                    <span>{agent.displayName}</span>
                    <span className="usage-sync-agent-state" data-tone={state.tone}>
                      <span className="usage-sync-agent-dot" aria-hidden="true" />
                      {state.text}
                    </span>
                  </div>
                  <button type="button" className="btn-sm" aria-label={`重新采集 ${agent.displayName}`}
                    disabled={recollecting} onClick={() => onRecollect(agent.id)}>重新采集</button>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}
