import type { Project, SessionMeta } from "./types";
import { sessionKey } from "./utils.ts";

// 仅用于侧栏选择；请求后端时仍传递 SessionMeta 原有的 projectDir。
export const PROJECTLESS_GROUP = "group:projectless";
export interface SessionGroup extends Project { projectless: boolean }
const NO_HINTS: ReadonlyMap<string, string | null> = new Map();

export function sessionGroup(session: SessionMeta, projectlessIds: ReadonlySet<string>, hints: ReadonlyMap<string, string | null> = NO_HINTS): string {
  const root = hints.get(sessionKey(session));
  if (root !== undefined) return root === null ? PROJECTLESS_GROUP : `group:root:${root}`;
  return projectlessIds.has(session.sessionId) ? PROJECTLESS_GROUP : session.projectDir;
}

export function groupSessions(projects: Project[], sessions: SessionMeta[], projectlessIds: ReadonlySet<string>, hints: ReadonlyMap<string, string | null> = NO_HINTS): SessionGroup[] {
  if (projectlessIds.size === 0 && hints.size === 0) return projects.map((project) => ({ ...project, projectless: false }));
  const paths = new Map(projects.map((project) => [project.dirName, project.displayPath]));
  const groups = new Map<string, SessionGroup>();
  for (const session of sessions) {
    const key = sessionGroup(session, projectlessIds, hints);
    const projectless = key === PROJECTLESS_GROUP;
    let group = groups.get(key);
    if (!group) {
      group = { dirName: key, displayPath: projectless ? "未归属项目" : hints.get(sessionKey(session)) ?? paths.get(key) ?? session.cwd ?? session.projectDir,
        projectless, sessionCount: 0, lastActivity: null };
      groups.set(key, group);
    }
    group.sessionCount++;
    if (session.lastTs && (!group.lastActivity || session.lastTs > group.lastActivity)) group.lastActivity = session.lastTs;
  }
  return [...groups.values()].sort((a,b) => Number(b.projectless) - Number(a.projectless)
    || (b.lastActivity ?? "").localeCompare(a.lastActivity ?? ""));
}
