import { Folder, Folders, FolderX } from "lucide-react";
import type { SessionGroup } from "../sessionGroups";
import { relativeTime, shortPath } from "../utils";
import PathLabel from "./PathLabel";
import UiIcon from "./UiIcon";

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
          type="button"
          className={`project-item${selected === null ? " active" : ""}`}
          disabled={disabled}
          onClick={() => onSelect(null)}
        >
          <div className="project-heading">
            <UiIcon icon={Folders} className="project-icon" />
            <div className="project-path">全部会话</div>
          </div>
          {(!loading || total > 0) && <div className="project-meta">{countLabel}</div>}
        </button>
      </div>
      <div className="sidebar-project-list" key={selectedAgent}>
        {projects.map((p) => {
          const icon = p.projectless ? Folders : p.missing ? FolderX : Folder;
          const title = p.projectless ? undefined : p.missing ? `${p.displayPath}（目录不存在）` : p.displayPath;
          return (
            <button
              type="button"
              key={p.dirName}
              className={`project-item${selected === p.dirName ? " active" : ""}${p.missing ? " missing" : ""}`}
              disabled={disabled}
              onClick={() => onSelect(p.dirName)}
              title={title}
            >
              <div className="project-heading">
                <UiIcon icon={icon} className="project-icon" />
                <PathLabel path={shortPath(p.displayPath) || p.dirName} className="project-path" fontKey={selected === p.dirName} />
              </div>
              <div className="project-meta">
                <span>{p.sessionCount} 个会话</span>
                <time dateTime={p.lastActivity ?? undefined}>{relativeTime(p.lastActivity)}</time>
              </div>
            </button>
          );
        })}
        {!loading && projects.length === 0 && <div className="empty-hint">{catalogIncomplete ? "会话列表读取失败" : "暂无会话"}</div>}
      </div>
    </aside>
  );
}
