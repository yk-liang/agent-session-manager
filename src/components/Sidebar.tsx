import type { SessionGroup } from "../sessionGroups";
import { relativeTime, shortPath } from "../utils";

interface Props {
  selectedAgent: string | null;
  projects: SessionGroup[];
  selected: string | null;
  onSelect: (dirName: string | null) => void;
  disabled?: boolean;
  loading: boolean;
  /** 扫描不完整时无法确认总数。 */
  catalogIncomplete?: boolean;
}

export default function Sidebar({
  selectedAgent,
  projects,
  selected,
  onSelect,
  disabled = false,
  loading,
  catalogIncomplete = false,
}: Props) {
  const total = projects.reduce((n, p) => n + p.sessionCount, 0);
  const countLabel = catalogIncomplete
    ? (total > 0 ? `已列出 ${total} 个会话` : "会话数量暂不可确认")
    : `${total} 个会话`;
  return (
    <aside className="sidebar">
      <div className="sidebar-projects-header">
        <div className="sidebar-title">项目</div>
        <button
          className={`project-item ${selected === null ? "active" : ""}`}
          disabled={disabled}
          onClick={() => onSelect(null)}
        >
          <div className="project-path">全部会话</div>
          {(!loading || total > 0) && <div className="project-meta">{countLabel}</div>}
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
              <span>{p.sessionCount} 个会话</span>
              <time dateTime={p.lastActivity ?? undefined}>{relativeTime(p.lastActivity)}</time>
            </div>
          </button>
        ))}
        {!loading && projects.length === 0 && <div className="empty-hint">{catalogIncomplete ? "会话列表读取失败" : "暂无会话"}</div>}
      </div>
    </aside>
  );
}
