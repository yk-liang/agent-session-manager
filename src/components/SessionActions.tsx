import { Archive, ArchiveRestore, Copy, Terminal, Trash2, type LucideIcon } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import * as api from "../api";
import type { AgentInfo, SessionMeta } from "../types";
import UiIcon from "./UiIcon";

interface Props {
  agent: AgentInfo;
  session: SessionMeta;
  onRequestDelete: (session: SessionMeta) => void;
  onToggleArchive: (session: SessionMeta) => void;
  toast: (message: string, error?: boolean) => void;
  compact?: boolean;
  disabled: boolean;
}

export default function SessionActions({ agent, session, onRequestDelete, onToggleArchive, toast, compact = false, disabled }: Props) {
  const canResume = !session.archived && session.hasTranscript && (!agent.resumeEntrypoint || session.entrypoint === agent.resumeEntrypoint);
  const canDelete = agent.deletePolicy !== "readOnly";
  if (!canResume && !canDelete && !agent.supportsArchive) return null;

  const copyResume = async () => {
    try {
      const command = await api.resumeCommand(agent.id, session.sessionId);
      if (!command) {
        toast("该 Agent 不支持恢复会话", true);
        return;
      }
      await writeText(command);
      toast("已复制恢复命令");
    } catch (error) {
      toast(String(error), true);
    }
  };

  const resume = async (fork: boolean) => {
    try {
      await api.resumeInTerminal(agent.id, session.sessionId, session.cwd ?? undefined, fork);
      toast(fork ? "已向终端发送分叉命令" : "已向终端发送恢复命令");
    } catch (error) {
      toast(String(error), true);
    }
  };

  const button = (label: string, icon: LucideIcon, onClick: () => void, { danger = false, showIcon = false } = {}) => (
    <button type="button" className={danger ? "danger" : undefined} disabled={disabled}
      title={!compact && showIcon ? label : undefined}
      aria-label={compact ? `${label}（${session.title || session.sessionId}）` : undefined} onClick={onClick}>
      {(compact || showIcon) && <UiIcon icon={icon} />}
      {compact ? <span className="session-action-tooltip" aria-hidden="true">{label}</span> : label}
    </button>
  );

  const buttons = (
    <>
      {canResume && button("复制恢复命令", Copy, copyResume)}
      {canResume && button("在终端恢复", Terminal, () => resume(false))}
      {canResume && !compact && agent.supportsFork && (
        <button type="button" disabled={disabled} onClick={() => resume(true)}>分叉会话</button>
      )}
      {agent.supportsArchive && button(session.archived ? "取消归档" : "归档",
        session.archived ? ArchiveRestore : Archive, () => onToggleArchive(session), { showIcon: true })}
      {canDelete && button("删除", Trash2, () => onRequestDelete(session), { danger: true })}
    </>
  );

  return compact ? <div className="session-card-actions" role="group" aria-label="会话快捷操作">{buttons}</div> : buttons;
}
