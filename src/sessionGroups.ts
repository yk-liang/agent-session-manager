import type { Catalog, Project, SessionMeta } from "./types";
import { compareTimestamps, sessionKey } from "./utils.ts";

export const PROJECTLESS_GROUP = "group:projectless";
export interface SessionGroup extends Project {
  projectless: boolean;
  missing: boolean;
}

function buildCatalogGroups(catalog: Catalog) {
  const projectlessIds = new Set(catalog.projectlessSessionIds);
  const hints = new Map(catalog.sessionGroupHints?.map(hint => [sessionKey(hint), hint.root]));
  const regroup = projectlessIds.size > 0 || hints.size > 0;
  const paths = new Map(catalog.projects.map(project => [project.dirName, project.displayPath]));
  const missingPaths = new Set(catalog.missingProjectPaths);
  const byGroup = new Map<string, SessionMeta[]>();
  const projectlessKeys = new Set<string>();
  const groups = new Map<string, SessionGroup>();

  for (const session of catalog.sessions) {
    const key = sessionKey(session);
    const root = hints.get(key);
    const projectless = root === null || root === undefined && projectlessIds.has(session.sessionId);
    const dirName = projectless ? PROJECTLESS_GROUP : root === undefined ? session.projectDir : `group:root:${root}`;
    const rows = byGroup.get(dirName);
    if (rows) rows.push(session);
    else byGroup.set(dirName, [session]);
    if (projectless) projectlessKeys.add(key);
    if (!regroup) continue;

    const group = groups.get(dirName);
    if (!group) {
      const displayPath = projectless ? "未归属项目" : root ?? paths.get(dirName) ?? session.cwd ?? session.projectDir;
      groups.set(dirName, {
        dirName, displayPath, projectless, missing: !projectless && missingPaths.has(displayPath),
        sessionCount: 1, lastActivity: session.lastTs,
      });
      continue;
    }
    group.sessionCount++;
    if (compareTimestamps(session.lastTs, group.lastActivity) > 0) group.lastActivity = session.lastTs;
  }

  return {
    groups: regroup ? [...groups.values()].sort((a, b) => Number(b.projectless) - Number(a.projectless)
      || compareTimestamps(b.lastActivity, a.lastActivity))
      : catalog.projects.map(project => ({ ...project, projectless: false, missing: missingPaths.has(project.displayPath) })),
    byGroup, hints, projectlessKeys,
  };
}

const catalogGroupCache = new WeakMap<Catalog, ReturnType<typeof buildCatalogGroups>>();

export function catalogGroups(catalog: Catalog) {
  let view = catalogGroupCache.get(catalog);
  if (!view) {
    view = buildCatalogGroups(catalog);
    catalogGroupCache.set(catalog, view);
  }
  return view;
}
