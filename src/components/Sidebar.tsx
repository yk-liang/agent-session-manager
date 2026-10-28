import type { AgentInfo } from "../types";
import type { SessionGroup } from "../sessionGroups";
import { relativeTime, shortPath } from "../utils";
import AgentIcon from "./AgentIcon";

interface Props {
  agents: AgentInfo[];
  selectedAgent: string | null;
  onSelectAgent: (id: string) => void;
  projects: SessionGroup[];
  selected: string | null;
  onSelect: (dirName: string | null) => void;
  // 配置页只需选 agent：隐藏项目列表，agent 列表始终显示
  hideProjects?: boolean;
  disabled?: boolean;
}

export default function Sidebar({
  agents,
  selectedAgent,
  onSelectAgent,
  projects,
  selected,
  onSelect,
  hideProjects = false,
  disabled = false,
}: Props) {
  const total = projects.reduce((n, p) => n + p.sessionCount, 0);
  return (
    <aside className="sidebar">
      {(hideProjects || agents.length > 1) && (
        <div className="sidebar-agents">
          <div className="sidebar-title">Agent</div>
          {agents.map((a) => (
            <button
              key={a.id}
              className={`project-item agent-item ${selectedAgent === a.id ? "active" : ""}`}
              disabled={disabled}
              onClick={() => onSelectAgent(a.id)}
            >
              <AgentIcon agent={a.id} />
              <div className="project-path">{a.displayName}</div>
            </button>
          ))}
          {agents.length === 0 && <div className="empty-hint">未发现已安装的 Agent</div>}
        </div>
      )}
      {!hideProjects && (
        <>
          <div className={`sidebar-projects-header ${agents.length > 1 ? "sidebar-divider" : ""}`}>
            <div className="sidebar-title">项目</div>
            <button
              className={`project-item ${selected === null ? "active" : ""}`}
              disabled={disabled}
              onClick={() => onSelect(null)}
            >
              <div className="project-path">全部会话</div>
              <div className="project-meta">{total} 个会话</div>
            </button>
          </div>
          <div className="sidebar-project-list" key={selectedAgent}>
            {projects.map((p) => (
              <button
                key={p.dirName}
                className={`project-item ${selected === p.dirName ? "active" : ""}`}
                disabled={disabled}
                onClick={() => onSelect(p.dirName)}
                title={p.projectless ? undefined : p.displayPath}
              >
                <div className="project-path">{shortPath(p.displayPath) || p.dirName}</div>
                <div className="project-meta">
                  {p.sessionCount} 个会话 · {relativeTime(p.lastActivity)}
                </div>
              </button>
            ))}
            {projects.length === 0 && <div className="empty-hint">未发现会话</div>}
          </div>
        </>
      )}
    </aside>
  );
}
