import type { Catalog, CatalogFilter, Project, SessionMeta } from "./types";
import { sessionKey } from "./utils.ts";

export const EMPTY_CATALOG: Catalog = { projects: [], sessions: [], warnings: [] };
export const DEFAULT_CATALOG_FILTER: CatalogFilter = { includeEmpty: false, includeArchived: false };
type CatalogLoader = (agent: string, filter: CatalogFilter, generation: number, revision?: number) => Promise<Catalog | null>;
const scopeKey = (agent: string, { includeEmpty, includeArchived }: CatalogFilter) =>
  JSON.stringify([agent, includeEmpty, includeArchived]);

const matchesFilter = (session: SessionMeta, filter: CatalogFilter) =>
  (filter.includeArchived || !session.archived)
  && (filter.includeEmpty || session.realUserCount > 0 || session.statsPending);

function withSessions(catalog: Catalog, sessions: SessionMeta[], previous?: Catalog): Catalog {
  const paths = new Map(previous?.projects.map(project => [project.dirName, project.displayPath]));
  for (const project of catalog.projects) paths.set(project.dirName, project.displayPath);
  const groups = new Map<string, Project>();
  for (const session of sessions) {
    const group = groups.get(session.projectDir);
    if (group) {
      group.sessionCount++;
      if (session.lastTs && (!group.lastActivity || session.lastTs > group.lastActivity)) group.lastActivity = session.lastTs;
    } else {
      groups.set(session.projectDir, {
        dirName: session.projectDir,
        displayPath: paths.get(session.projectDir) ?? session.cwd ?? session.projectDir,
        sessionCount: 1,
        lastActivity: session.lastTs,
      });
    }
  }
  const projects = [...groups.values()].sort((a, b) =>
    (b.lastActivity ?? "").localeCompare(a.lastActivity ?? "") || a.dirName.localeCompare(b.dirName));
  return { ...catalog, projects, sessions };
}

interface CatalogScope {
  agent: string;
  filter: CatalogFilter;
  sequence: number;
  snapshot?: Catalog;
  pending?: { generation: number; request: Promise<Catalog> };
}

export class AgentCatalogCache {
  private scopes = new Map<string, CatalogScope>();
  private readFailures = new Map<string, Map<string, SessionMeta>>();
  private listeners = new Set<() => void>();
  private loader: CatalogLoader;

  constructor(loader: CatalogLoader) {
    this.loader = loader;
  }

  private scope(agent: string, filter: CatalogFilter) {
    const key = scopeKey(agent, filter);
    let scope = this.scopes.get(key);
    if (!scope) {
      scope = { agent, filter, sequence: 0 };
      this.scopes.set(key, scope);
    }
    return scope;
  }

  peek(agent: string, filter: CatalogFilter) {
    return this.scopes.get(scopeKey(agent, filter))?.snapshot;
  }

  visible(agent: string, filter: CatalogFilter) {
    return this.peek(agent, filter)
      ?? (filter.includeEmpty ? this.peek(agent, { ...filter, includeEmpty: false }) : undefined)
      ?? (filter.includeArchived ? this.peek(agent, { ...filter, includeArchived: false }) : undefined)
      ?? (filter.includeEmpty && filter.includeArchived ? this.peek(agent, DEFAULT_CATALOG_FILTER) : undefined);
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  private publish(scope: CatalogScope, catalog: Catalog) {
    if (scope.snapshot !== catalog) {
      scope.snapshot = catalog;
      for (const listener of this.listeners) listener();
    }
    return catalog;
  }

  withCurrentMetadata(agent: string, filter: CatalogFilter, meta: SessionMeta): SessionMeta {
    const snapshot = this.visible(agent, filter);
    const key = sessionKey(meta);
    const latest = snapshot?.sessions.find(session => sessionKey(session) === key);
    if (!latest) return meta;
    const title = latest.statsPending ? latest.title ?? meta.title : latest.title;
    const model = latest.statsPending ? meta.model : latest.model;
    const archived = latest.archived ?? false;
    return meta.title === title && meta.model === model && (meta.archived ?? false) === archived
      ? meta : { ...meta, title, model, archived };
  }

  private decorate({ agent, filter, snapshot }: CatalogScope, catalog: Catalog): Catalog {
    const failures = this.readFailures.get(agent);
    const sessionKeys = catalog.warnings.length && failures?.size ? new Set(catalog.sessions.map(sessionKey)) : null;
    const restored: SessionMeta[] = [];
    // 扫描也失败时保留已知失败会话的摘要，不能让正文打不开的卡片随刷新消失。
    if (sessionKeys) {
      for (const failed of failures?.values() ?? []) {
        if (matchesFilter(failed, filter) && !sessionKeys.has(sessionKey(failed))) {
          restored.push(failed);
          sessionKeys.add(sessionKey(failed));
        }
      }
    }
    const rows = restored.length ? [...catalog.sessions, ...restored] : catalog.sessions;
    const sessions = rows.map(session => {
      const readError = failures?.size ? failures.get(sessionKey(session))?.readError : undefined;
      return readError === session.readError ? session : { ...session, readError };
    });
    if (restored.length) return withSessions(catalog, sessions, snapshot);
    return sessions.some((session, index) => session !== catalog.sessions[index])
      ? { ...catalog, sessions } : catalog;
  }

  /** 只有同一会话的正文重试成功才清除失败；名称补全和摘要缓存命中都不代表正文可读。 */
  setReadError(agent: string, session: SessionMeta, error: string | null) {
    const failures = this.readFailures.get(agent) ?? new Map<string, SessionMeta>();
    if (failures.get(sessionKey(session))?.readError === (error ?? undefined)) return;
    if (error !== null) failures.set(sessionKey(session), {...session, readError: error});
    else failures.delete(sessionKey(session));
    this.readFailures.set(agent, failures);
    for (const scope of this.scopes.values()) {
      if (scope.agent === agent && scope.snapshot) this.publish(scope, this.decorate(scope, scope.snapshot));
    }
  }

  load(agent: string, filter: CatalogFilter, generation: number): Promise<Catalog> {
    const scope = this.scope(agent, filter);
    if (scope.pending && scope.pending.generation === generation) return scope.pending.request;
    const sequence = ++scope.sequence;
    const request = this.loader(agent, filter, generation, scope.snapshot?.revision).then(catalog => {
      if (catalog && sequence === scope.sequence) return this.publish(scope, this.decorate(scope, catalog));
      return scope.snapshot ?? EMPTY_CATALOG;
    }).finally(() => {
      if (scope.pending?.request === request) scope.pending = undefined;
    });
    scope.pending = { generation, request };
    return request;
  }

  async refine(agent: string, filter: CatalogFilter, load: (revision?: number) => Promise<Catalog | null>) {
    const scope = this.scope(agent, filter);
    const sequence = scope.sequence;
    const catalog = await load(scope.snapshot?.revision);
    if (catalog && sequence === scope.sequence) return this.publish(scope, this.decorate(scope, catalog));
  }

  remove(agent: string, removed: SessionMeta[]) {
    const keys = new Set(removed.map(sessionKey));
    for (const key of keys) this.readFailures.get(agent)?.delete(key);
    this.updateSnapshots(agent, catalog => withSessions(catalog, catalog.sessions.filter(row => !keys.has(sessionKey(row)))));
  }

  setArchived(agent: string, session: SessionMeta, archived: boolean) {
    const key = sessionKey(session);
    const failures = this.readFailures.get(agent);
    const failed = failures?.get(key);
    if (failed) failures?.set(key, { ...failed, archived });
    this.updateSnapshots(agent, (catalog, filter) => {
      const previous = catalog.sessions.find(row => sessionKey(row) === key);
      const updated = { ...(previous ?? session), archived };
      if (!matchesFilter(updated, filter)) {
        return previous ? withSessions(catalog, catalog.sessions.filter(row => row !== previous)) : catalog;
      }
      if (previous) {
        const sessions = catalog.sessions.map(row => row === previous ? updated : row);
        return { ...catalog, sessions };
      }
      const sessions = [...catalog.sessions, updated].sort((a, b) =>
        (b.lastTs ?? "").localeCompare(a.lastTs ?? "") || a.projectDir.localeCompare(b.projectDir)
        || a.sessionId.localeCompare(b.sessionId));
      return withSessions(catalog, sessions);
    });
  }

  private updateSnapshots(agent: string, update: (catalog: Catalog, filter: CatalogFilter) => Catalog) {
    for (const scope of this.scopes.values()) {
      if (scope.agent !== agent) continue;
      scope.sequence++;
      scope.pending = undefined;
      if (!scope.snapshot) continue;
      const updated = update(scope.snapshot, scope.filter);
      if (updated !== scope.snapshot) this.publish(scope, { ...updated, revision: undefined });
    }
  }
}
