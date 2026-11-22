import { useLayoutEffect, useRef, useState } from "react";
import { Folder, Folders, FolderX } from "lucide-react";
import type { SessionGroup } from "../sessionGroups";
import { relativeTime, shortPath } from "../utils";
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

let pathMeasure: CanvasRenderingContext2D | null = null;

function fitProjectPath(path: string, fits: (label: string) => boolean) {
  if (fits(path)) return path;

  const parts = path.split("/");
  const trailingSlash = path.endsWith("/");
  if (trailingSlash) parts.pop();
  const leaf = parts.pop();
  if (!parts.length || !leaf) return path;

  const suffix = trailingSlash ? `${leaf}/` : leaf;
  const compact = [...parts];
  for (const [index, part] of parts.entries()) {
    if (!part || part === "~" || part === "." || part === "..") continue;
    compact[index] = Array.from(part).slice(0, part.startsWith(".") ? 2 : 1).join("");
    const label = [...compact, suffix].join("/");
    if (fits(label)) return label;
  }
  for (let start = 1; start <= parts.length; start++) {
    const label = ["…", ...parts.slice(start), suffix].join("/");
    if (fits(label)) return label;
  }
  return suffix;
}

function ProjectPath({ path, active }: { path: string; active: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const [label, setLabel] = useState(path);

  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = pathMeasure ??= document.createElement("canvas").getContext("2d");
    if (!measure) return;
    const update = () => {
      const { width } = element.getBoundingClientRect();
      const { fontFamily, fontWeight, fontSize } = getComputedStyle(element);
      measure.font = `${fontWeight} ${fontSize} ${fontFamily}`;
      setLabel(fitProjectPath(path, text => measure.measureText(text).width <= width));
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, [path, active]);

  return <div ref={ref} className="project-path" aria-label={path}>{label}</div>;
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
                <ProjectPath path={shortPath(p.displayPath) || p.dirName} active={selected === p.dirName} />
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
