import type { AgentInfo, SessionMeta } from "../types";
import { displaySessionId, sessionKey, shortPath } from "../utils";
import ConfirmDialog from "./ConfirmDialog";

export type SessionDeleteTarget =
  | { mode: "single"; sessions: [SessionMeta] }
  | { mode: "batch"; sessions: SessionMeta[] };

export type SessionDeletion = SessionDeleteTarget & { agent: AgentInfo };

interface Props {
  deletion: SessionDeletion;
  onConfirm: () => void;
  onCancel: () => void;
}

const deletesChildSessions = new Set(["claude", "codex", "opencode", "pi", "qoder"]);

export default function SessionDeleteDialog({ deletion, onConfirm, onCancel }: Props) {
  const { agent, sessions } = deletion;
  const batch = deletion.mode === "batch";
  const toTrash = agent.deletePolicy === "trash";
  const confirmLabel = toTrash ? "移入废纸篓" : "永久删除";
  const session = sessions[0];
  const cascades = agent.id === "codex";

  return (
    <ConfirmDialog
      title={batch ? "批量删除会话" : "删除会话"}
      message={
        <>
          {batch ? (cascades
            ? <p>已选 <strong className="deletion-count">{sessions.length} 项</strong>，每项包含所列会话及其所有后代。</p>
            : <p>将删除 {agent.displayName} 的 <strong className="deletion-count">{sessions.length} 个会话</strong>。</p>)
            : <p>「{session.title || displaySessionId(session)}」</p>}
          <p>{toTrash ? "可从废纸篓还原。" : "删除后无法恢复。"}</p>
          {deletesChildSessions.has(agent.id) && !(batch && cascades) && <p>子会话也会一并删除。</p>}
          {batch && (
            <>
              <ul className="batch-targets">
                {sessions.map((target) => (
                  <li key={sessionKey(target)}>
                    <span>{target.title || displaySessionId(target)}</span>
                    <small className="session-location"><span>{shortPath(target.cwd) || target.projectDir}</span><code>{displaySessionId(target)}</code></small>
                  </li>
                ))}
              </ul>
            </>
          )}
        </>
      }
      confirmLabel={batch ? `${confirmLabel}（${sessions.length}${cascades ? " 项" : ""}）` : confirmLabel}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}
