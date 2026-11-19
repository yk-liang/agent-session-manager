import type { AgentInfo, SessionMeta } from "../types";
import { sessionKey, shortPath } from "../utils";
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

export default function SessionDeleteDialog({ deletion, onConfirm, onCancel }: Props) {
  const { agent, sessions } = deletion;
  const batch = deletion.mode === "batch";
  const toTrash = agent.deletePolicy === "trash";
  const confirmLabel = toTrash ? "移入废纸篓" : "永久删除";
  const session = sessions[0];

  return (
    <ConfirmDialog
      title={batch ? "批量删除会话" : "删除会话"}
      message={
        <>
          <p>{batch
            ? `${agent.displayName} · ${sessions.length} 个会话`
            : `「${session.title || session.sessionId.slice(0, 8) + "…"}」`}</p>
          <p>{toTrash ? "会话文件将移入废纸篓，可在访达中还原。" : "会话将永久删除，无法恢复。"}</p>
          {agent.id === "opencode" && <p>关联的 OpenCode 子会话也会被删除。</p>}
          {batch && (
            <>
              <ul className="batch-targets">
                {sessions.map((target) => (
                  <li key={sessionKey(target)}>
                    {target.title || target.sessionId}
                    <small>{shortPath(target.cwd) || target.projectDir} · {target.sessionId}</small>
                  </li>
                ))}
              </ul>
              <p>应用会列出删除失败的会话，供你重试。</p>
            </>
          )}
        </>
      }
      confirmLabel={batch ? `${confirmLabel}（${sessions.length}）` : confirmLabel}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  );
}
