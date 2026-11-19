import type { Catalog, Project, SessionMeta } from "./types";
import { sessionKey } from "./utils.ts";

export const EMPTY_CATALOG: Catalog = { projects: [], sessions: [], warnings: [] };
type CatalogLoader = (agent: string, includeEmpty: boolean, generation: number, revision?: number) => Promise<Catalog | null>;

interface CatalogScope {
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

  private scope(agent: string, includeEmpty: boolean) {
    const key = JSON.stringify([agent, includeEmpty]);
    let scope = this.scopes.get(key);
    if (!scope) {
      scope = { sequence: 0 };
      this.scopes.set(key, scope);
    }
    return scope;
  }

  peek(agent: string, includeEmpty: boolean) {
    return this.scopes.get(JSON.stringify([agent, includeEmpty]))?.snapshot;
  }

  visible(agent: string, includeEmpty: boolean) {
    return this.peek(agent, includeEmpty) ?? (includeEmpty ? this.peek(agent, false) : undefined);
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

  withCurrentMetadata(agent: string, includeEmpty: boolean, meta: SessionMeta): SessionMeta {
    const snapshot = this.visible(agent, includeEmpty);
    const key = sessionKey(meta);
    const latest = snapshot?.sessions.find(session => sessionKey(session) === key);
    if (!latest) return meta;
    const title = latest.statsPending ? latest.title ?? meta.title : latest.title;
    const model = latest.statsPending ? meta.model : latest.model;
    return meta.title === title && meta.model === model ? meta : { ...meta, title, model };
  }

  private decorate(agent: string, includeEmpty: boolean, catalog: Catalog): Catalog {
    const failures = this.readFailures.get(agent);
    const sessionKeys = catalog.warnings.length && failures?.size ? new Set(catalog.sessions.map(sessionKey)) : null;
    const restored: SessionMeta[] = [];
    // 扫描也失败时保留已知失败会话的摘要，不能让正文打不开的卡片随刷新消失。
    if (sessionKeys) {
      for (const failed of failures?.values() ?? []) {
        if ((includeEmpty || failed.realUserCount > 0 || failed.statsPending) && !sessionKeys.has(sessionKey(failed))) {
          restored.push(failed);
          sessionKeys.add(sessionKey(failed));
        }
      }
    }
    let projects = catalog.projects;
    if (restored.length) {
      const counts = new Map(projects.map(project => [project.dirName, project]));
      const previous = new Map(this.peek(agent, includeEmpty)?.projects.map(project => [project.dirName, project]));
      for (const session of restored) {
        const project = counts.get(session.projectDir);
        const metadata = project ?? previous.get(session.projectDir) ?? {
          dirName: session.projectDir, displayPath: session.cwd ?? session.projectDir, lastActivity: session.lastTs,
        };
        counts.set(session.projectDir, { ...metadata, sessionCount: (project?.sessionCount ?? 0) + 1 });
      }
      projects = [...counts.values()];
    }
    const rows = restored.length ? [...catalog.sessions, ...restored] : catalog.sessions;
    const sessions = rows.map(session => {
      const readError = failures?.size ? failures.get(sessionKey(session))?.readError : undefined;
      return readError === session.readError ? session : { ...session, readError };
    });
    return restored.length || sessions.some((session, index) => session !== catalog.sessions[index])
      ? { ...catalog, projects, sessions } : catalog;
  }

  /** 只有同一会话的正文重试成功才清除失败；名称补全和摘要缓存命中都不代表正文可读。 */
  setReadError(agent: string, session: SessionMeta, error: string | null) {
    const failures = this.readFailures.get(agent) ?? new Map<string, SessionMeta>();
    if (failures.get(sessionKey(session))?.readError === (error ?? undefined)) return;
    if (error !== null) failures.set(sessionKey(session), {...session, readError: error});
    else failures.delete(sessionKey(session));
    this.readFailures.set(agent, failures);
    for (const empty of [false, true]) {
      const scope = this.scope(agent, empty);
      if (scope.snapshot) this.publish(scope, this.decorate(agent, empty, scope.snapshot));
    }
  }

  load(agent: string, includeEmpty: boolean, generation: number): Promise<Catalog> {
    const scope = this.scope(agent, includeEmpty);
    if (scope.pending && scope.pending.generation === generation) return scope.pending.request;
    const sequence = ++scope.sequence;
    const request = this.loader(agent, includeEmpty, generation, scope.snapshot?.revision).then(catalog => {
      if (catalog && sequence === scope.sequence) return this.publish(scope, this.decorate(agent, includeEmpty, catalog));
      return scope.snapshot ?? EMPTY_CATALOG;
    }).finally(() => {
      if (scope.pending?.request === request) scope.pending = undefined;
    });
    scope.pending = { generation, request };
    return request;
  }

  async refine(agent: string, includeEmpty: boolean, load: (revision?: number) => Promise<Catalog | null>) {
    const scope = this.scope(agent, includeEmpty);
    const sequence = scope.sequence;
    const catalog = await load(scope.snapshot?.revision);
    if (catalog && sequence === scope.sequence) return this.publish(scope, this.decorate(agent, includeEmpty, catalog));
  }

  remove(agent: string, removed: SessionMeta[]) {
    const keys = new Set(removed.map(sessionKey));
    for (const key of keys) this.readFailures.get(agent)?.delete(key);
    for (const empty of [false, true]) {
      const scope = this.scope(agent, empty);
      scope.sequence++;
      scope.pending = undefined;
      const current = scope.snapshot;
      if (!current) continue;
      const sessions = current.sessions.filter((s) => !keys.has(sessionKey(s)));
      const groups = new Map<string, Project>();
      const projectsByDir = new Map(current.projects.map(project => [project.dirName, project]));
      for (const session of sessions) {
        const group = groups.get(session.projectDir);
        if (group) {
          group.sessionCount++;
          if (session.lastTs && (!group.lastActivity || session.lastTs > group.lastActivity)) group.lastActivity = session.lastTs;
        } else {
          const project = projectsByDir.get(session.projectDir);
          if (project) groups.set(session.projectDir, { ...project, sessionCount: 1, lastActivity: session.lastTs });
        }
      }
      const projects = current.projects.flatMap(project => groups.get(project.dirName) ?? []);
      this.publish(scope, { ...current, revision: undefined, projects, sessions });
    }
  }
}
