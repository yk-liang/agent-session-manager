import type { Catalog, CatalogFilter, Project, SessionMeta, SessionRef } from "./types";
import { compareTimestamps, isEmptySession, sessionKey } from "./utils.ts";

export const EMPTY_CATALOG: Catalog = { projects: [], sessions: [], warnings: [] };
export const DEFAULT_CATALOG_FILTER: CatalogFilter = { includeEmpty: false, includeArchived: false };
export function catalogFilterFor({ includeEmpty, includeArchived }: CatalogFilter, supportsArchivedSessions = false): CatalogFilter {
  return { includeEmpty, includeArchived: includeArchived && supportsArchivedSessions };
}

type CatalogLoader = (agent: string, filter: CatalogFilter, generation: number, revision?: number) => Promise<Catalog | null>;
const scopeKey = (agent: string, { includeEmpty, includeArchived }: CatalogFilter) =>
  JSON.stringify([agent, includeEmpty, includeArchived]);

const matchesFilter = (session: SessionMeta, filter: CatalogFilter) =>
  (filter.includeArchived || !session.archived)
  && (filter.includeEmpty || !isEmptySession(session) || session.statsPending);

function withSessions(catalog: Catalog, sessions: SessionMeta[], previous?: Catalog): Catalog {
  const paths = new Map(previous?.projects.map(project => [project.dirName, project.displayPath]));
  for (const project of catalog.projects) paths.set(project.dirName, project.displayPath);
  const groups = new Map<string, Project>();
  for (const session of sessions) {
    const group = groups.get(session.projectDir);
    if (group) {
      group.sessionCount++;
      if (compareTimestamps(session.lastTs, group.lastActivity) > 0) group.lastActivity = session.lastTs;
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
    compareTimestamps(b.lastActivity, a.lastActivity) || a.dirName.localeCompare(b.dirName));
  return { ...catalog, projects, sessions };
}

interface CatalogScope {
  agent: string;
  filter: CatalogFilter;
  sequence: number;
  snapshot?: Catalog;
  count?: number | null;
  countRequest?: Promise<void>;
  pending?: { generation: number; request: Promise<Catalog> };
}

export class AgentCatalogCache {
  private scopes = new Map<string, CatalogScope>();
  private readFailures = new Map<string, Map<string, SessionMeta>>();
  private listeners = new Set<() => void>();
  private countRevision = 0;
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

  count(agent: string, filter: CatalogFilter) {
    return this.scopes.get(scopeKey(agent, filter))?.count;
  }

  countVersion = () => this.countRevision;

  refreshCount(agent: string, filter: CatalogFilter, load: () => Promise<number | null>): Promise<void> {
    const scope = this.scope(agent, filter);
    if (scope.countRequest) return scope.countRequest;
    const { sequence, snapshot } = scope;
    const request = load().catch(() => null).then(count => {
      if (scope.sequence !== sequence || scope.snapshot !== snapshot) return;
      if (this.setCount(scope, count)) this.notify();
    }).finally(() => {
      if (scope.countRequest === request) scope.countRequest = undefined;
    });
    scope.countRequest = request;
    return request;
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

  private setCount(scope: CatalogScope, count: number | null) {
    if (scope.count === count) return false;
    scope.count = count;
    this.countRevision++;
    return true;
  }

  private notify() {
    for (const listener of this.listeners) listener();
  }

  private publish(scope: CatalogScope, catalog: Catalog, updateCount = true) {
    const countChanged = updateCount && !catalog.sessions.some(session => session.statsPending)
      && this.setCount(scope, catalog.warnings.length ? null : catalog.sessions.length);
    if (scope.snapshot !== catalog) {
      scope.snapshot = catalog;
      this.notify();
    } else if (countChanged) this.notify();
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
    const codex = latest.codex ?? meta.codex;
    return meta.title === title && meta.model === model && (meta.archived ?? false) === archived && meta.codex === codex
      ? meta : { ...meta, title, model, archived, codex };
  }

  /**
   * 预览只读文件头，标题、模型和消息数暂缺；后台统计完成前沿用同一来源会话上次确认的值，
   * 避免卡片在“有值 → 空 → 有值”之间跳动。预览带来的新值（如重命名、时间、大小、归档）照常生效。
   */
  private retainKnownStats(scope: CatalogScope, catalog: Catalog): Catalog {
    if (!catalog.sessions.some(session => session.statsPending)) return catalog;
    const previous = this.visible(scope.agent, scope.filter);
    if (!previous?.sessions.length) return catalog;
    const known = new Map<string, SessionMeta>();
    for (const session of previous.sessions) {
      if (!session.statsPending || session.statsRetained) known.set(sessionKey(session), session);
    }
    let changed = false;
    const sessions = catalog.sessions.map(session => {
      const last = session.statsPending ? known.get(sessionKey(session)) : undefined;
      if (!last) return session;
      changed = true;
      return {
        ...session,
        title: session.title ?? last.title,
        model: session.model ?? last.model,
        userCount: last.userCount,
        assistantCount: last.assistantCount,
        realUserCount: last.realUserCount,
        assistantTextCount: last.assistantTextCount,
        codex: session.codex ? { ...session.codex, empty: last.codex?.empty } : undefined,
        statsRetained: true,
      };
    });
    return changed ? { ...catalog, sessions } : catalog;
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
      if (scope.agent === agent && scope.snapshot) this.publish(scope, this.decorate(scope, scope.snapshot), false);
    }
  }

  load(agent: string, filter: CatalogFilter, generation: number): Promise<Catalog> {
    const scope = this.scope(agent, filter);
    if (scope.pending && scope.pending.generation === generation) return scope.pending.request;
    const sequence = ++scope.sequence;
    const request = this.loader(agent, filter, generation, scope.snapshot?.revision).then(catalog => {
      if (catalog && sequence === scope.sequence) return this.publish(scope, this.settle(scope, catalog));
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
    if (catalog && sequence === scope.sequence) return this.publish(scope, this.settle(scope, catalog));
  }

  private settle(scope: CatalogScope, catalog: Catalog) {
    return this.decorate(scope, this.retainKnownStats(scope, catalog));
  }

  remove(agent: string, removed: SessionRef[]) {
    const keys = new Set(removed.map(sessionKey));
    for (const key of keys) this.readFailures.get(agent)?.delete(key);
    this.updateSnapshots(agent, catalog => withSessions(catalog, catalog.sessions.filter(row => !keys.has(sessionKey(row)))));
  }

  setArchived(agent: string, changes: { session: SessionRef; archived: boolean; meta?: SessionMeta }[]) {
    if (!changes.length) return;
    const byKey = new Map(changes.map(change => [sessionKey(change.session), change]));
    const known = new Map<string, SessionMeta>();
    for (const scope of this.scopes.values()) {
      if (scope.agent !== agent) continue;
      for (const row of scope.snapshot?.sessions ?? []) {
        const key = sessionKey(row);
        if (byKey.has(key) && (!known.has(key) || !row.statsPending)) known.set(key, row);
      }
    }
    const failures = this.readFailures.get(agent);
    for (const [key, { archived }] of byKey) {
      const failed = failures?.get(key);
      if (failed) failures?.set(key, { ...failed, archived });
    }
    this.updateSnapshots(agent, (catalog, filter) => {
      const present = new Set<string>();
      const sessions = catalog.sessions.flatMap(row => {
        const key = sessionKey(row);
        const change = byKey.get(key);
        if (!change) return [row];
        present.add(key);
        const updated = { ...row, archived: change.archived };
        return matchesFilter(updated, filter) ? [updated] : [];
      });
      for (const [key, { archived, meta }] of byKey) {
        const row = !present.has(key) ? known.get(key) ?? meta : undefined;
        if (row && matchesFilter({ ...row, archived }, filter)) sessions.push({ ...row, archived });
      }
      if (sessions.length === catalog.sessions.length && sessions.every((row, index) => row === catalog.sessions[index])) {
        return catalog;
      }
      sessions.sort((a, b) => compareTimestamps(b.lastTs, a.lastTs) || a.projectDir.localeCompare(b.projectDir)
        || a.sessionId.localeCompare(b.sessionId));
      return withSessions(catalog, sessions);
    });
  }

  private updateSnapshots(agent: string, update: (catalog: Catalog, filter: CatalogFilter) => Catalog) {
    for (const scope of this.scopes.values()) {
      if (scope.agent !== agent) continue;
      scope.sequence++;
      scope.pending = undefined;
      scope.countRequest = undefined;
      if (!scope.snapshot) continue;
      const updated = update(scope.snapshot, scope.filter);
      if (updated !== scope.snapshot) this.publish(scope, { ...updated, revision: undefined });
    }
  }
}
