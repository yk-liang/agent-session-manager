import type { Catalog, SessionMeta } from "./types";
import { sessionKey } from "./utils.ts";

export const EMPTY_CATALOG: Catalog = { projects: [], sessions: [], warnings: [] };
type CatalogLoader = (agent: string, includeEmpty: boolean) => Promise<Catalog>;
type NameLoader = (agent: string, models: string[]) => Promise<Record<string, string>>;

/** 每个 Agent 的两种空会话筛选分别保存快照；并发刷新复用同一请求。 */
export class AgentCatalogCache {
  private snapshots = new Map<string, Catalog>();
  private pending = new Map<string, Promise<Catalog>>();
  private generations = new Map<string, number>();
  private revisions = new Map<string, number>();
  private names = new Map<string, Map<string, string>>();
  private pendingNames = new Map<string, Promise<void>>();
  private loader: CatalogLoader;
  private nameLoader: NameLoader;

  constructor(loader: CatalogLoader, nameLoader: NameLoader) {
    this.loader = loader;
    this.nameLoader = nameLoader;
  }

  private key(agent: string, includeEmpty: boolean) { return JSON.stringify([agent, includeEmpty]); }
  peek(agent: string, includeEmpty: boolean) { return this.snapshots.get(this.key(agent, includeEmpty)); }

  withNames(agent: string, meta: SessionMeta): SessionMeta {
    const name = meta.model ? this.names.get(agent)?.get(meta.model) : undefined;
    return name && name !== meta.modelName ? { ...meta, modelName: name } : meta;
  }

  private decorate(agent: string, catalog: Catalog): Catalog {
    const names = new Map(this.names.get(agent));
    for (const session of catalog.sessions) {
      if (session.model && session.modelName) names.set(session.model, session.modelName);
    }
    this.names.set(agent, names);
    return { ...catalog, sessions: catalog.sessions.map((session) => this.withNames(agent, session)) };
  }

  load(agent: string, includeEmpty: boolean): Promise<Catalog> {
    const key = this.key(agent, includeEmpty);
    const running = this.pending.get(key);
    if (running) return running;
    const generation = this.generations.get(agent) ?? 0;
    this.revisions.set(key, (this.revisions.get(key) ?? 0) + 1);
    const request = this.loader(agent, includeEmpty).then((catalog) => {
      // 删除完成前启动的扫描不能把已删除卡片写回缓存。
      if (generation !== (this.generations.get(agent) ?? 0)) return this.peek(agent, includeEmpty) ?? EMPTY_CATALOG;
      const next = this.decorate(agent, catalog);
      this.snapshots.set(key, next);
      return next;
    }).finally(() => { if (this.pending.get(key) === request) this.pending.delete(key); });
    this.pending.set(key, request);
    return request;
  }

  /** 精确统计完成前若又刷新或删除，旧响应不得覆盖新目录。 */
  async refine(agent: string, includeEmpty: boolean, loader: CatalogLoader): Promise<Catalog | undefined> {
    const key = this.key(agent, includeEmpty);
    const revision = this.revisions.get(key);
    const generation = this.generations.get(agent) ?? 0;
    const result = await loader(agent, includeEmpty);
    if (revision !== this.revisions.get(key) || generation !== (this.generations.get(agent) ?? 0)) return undefined;
    const next = this.decorate(agent, result);
    this.snapshots.set(key, next);
    return next;
  }

  async enrich(agent: string, includeEmpty: boolean, refreshKnown = false): Promise<Catalog | undefined> {
    const catalog = this.peek(agent, includeEmpty);
    // Qoder 的历史模型使用不透明 ID，需要外部名称目录；其他数据源本身携带可读标识。
    if (!catalog || (!refreshKnown && agent !== "qoder")) return catalog;
    const models = [...new Set(catalog.sessions.filter((s) => s.model && (refreshKnown || !s.modelName)).map((s) => s.model!))].sort();
    if (!models.length) return catalog;
    const key = JSON.stringify([agent, models]);
    let request = this.pendingNames.get(key);
    if (!request) {
      request = (async () => {
        for (let offset = 0; offset < models.length; offset += 256) {
          const resolved = await this.nameLoader(agent, models.slice(offset, offset + 256));
          const names = new Map(this.names.get(agent));
          for (const [id, name] of Object.entries(resolved)) names.set(id, name);
          this.names.set(agent, names);
        }
        // 补全结果只更新名称，不能用查询开始时的旧列表覆盖后来刷新、删除后的快照。
        for (const empty of [false, true]) {
          const current = this.peek(agent, empty);
          if (current) this.snapshots.set(this.key(agent, empty), {
            ...current, sessions: current.sessions.map((session) => this.withNames(agent, session)),
          });
        }
      })().finally(() => { this.pendingNames.delete(key); });
      this.pendingNames.set(key, request);
    }
    await request;
    return this.peek(agent, includeEmpty);
  }

  remove(agent: string, removed: SessionMeta[]) {
    const keys = new Set(removed.map(sessionKey));
    this.generations.set(agent, (this.generations.get(agent) ?? 0) + 1);
    for (const empty of [false, true]) {
      const key = this.key(agent, empty);
      this.pending.delete(key);
      const current = this.snapshots.get(key);
      if (!current) continue;
      const sessions = current.sessions.filter((s) => !keys.has(sessionKey(s)));
      const projects = current.projects.map((project) => {
        const rows = sessions.filter((s) => s.projectDir === project.dirName);
        return { ...project, sessionCount: rows.length,
          lastActivity: rows.map((s) => s.lastTs).filter((ts): ts is string => !!ts).sort().pop() ?? null };
      }).filter((project) => project.sessionCount > 0);
      this.snapshots.set(key, { ...current, projects, sessions });
    }
  }
}
