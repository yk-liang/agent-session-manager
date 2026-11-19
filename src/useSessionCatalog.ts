import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import * as api from "./api";
import { AutoRefreshScheduler } from "./autoRefresh";
import { AgentCatalogCache, catalogFilterFor, EMPTY_CATALOG } from "./catalogCache";
import { observeSessionChanges } from "./sessionWatch";
import type { AgentInfo, CatalogFilter } from "./types";

export function useSessionCounts(agents: AgentInfo[], filter: CatalogFilter, cache: AgentCatalogCache, activeAgent: string | null) {
  useSyncExternalStore(cache.subscribe, cache.countVersion);
  const scopes = agents.map(agent => ({ id: agent.id, filter: catalogFilterFor(filter, agent.supportsArchive) }));
  return {
    counts: new Map(scopes.map(({ id, filter }) => [id, cache.count(id, filter) ?? null] as const)),
    refresh() {
      for (const { id, filter } of scopes) {
        if (id !== activeAgent) void cache.refreshCount(id, filter, () => api.countSessions(id, filter));
      }
    },
  };
}

export function useSessionCatalog(
  agent: AgentInfo | undefined,
  filter: CatalogFilter,
  active: boolean,
  blocked: boolean,
  onWatchError: (error: string) => void,
) {
  const [cache] = useState(() => new AgentCatalogCache(api.catalog));
  const [refreshKey, setRefreshKey] = useState(0);
  const scope = useMemo(() => ({ agent, filter }), [agent, filter]);
  const catalog = useSyncExternalStore(cache.subscribe, () => active && agent ? cache.visible(agent.id, filter) : undefined);
  const [status, setStatus] = useState<{ scope: typeof scope; refreshing: boolean; error: string | null }>();
  const scheduler = useRef<AutoRefreshScheduler | null>(null);
  const blockedRef = useRef(blocked);
  blockedRef.current = blocked;

  useEffect(() => {
    if (!scope.agent || !active) return;
    const { id } = scope.agent;
    const { filter } = scope;
    let disposed = false;
    const autoRefresh = new AutoRefreshScheduler(reload);
    scheduler.current = autoRefresh;
    autoRefresh.setBlocked(blockedRef.current);

    async function reload() {
      const generation = api.nextCatalogGeneration();
      autoRefresh.setBusy(true);
      setStatus({ scope, refreshing: true, error: null });
      try {
        const catalog = await cache.load(id, filter, generation);
        if (disposed) return;
        setStatus({ scope, refreshing: false, error: null });
        if (catalog.sessions.some(session => session.statsPending)) {
          await cache.refine(id, filter, async revision => {
            const refined = await api.refineCatalog(id, filter, generation, revision);
            if (disposed) return null;
            if (blockedRef.current) {
              autoRefresh.request("focus");
              return null;
            }
            return refined;
          }).catch(() => {});
        }
      } catch (error) {
        if (!disposed) setStatus({ scope, refreshing: false, error: String(error) });
      } finally {
        autoRefresh.setBusy(false);
      }
    }

    const stopObserving = observeSessionChanges(id, autoRefresh, onWatchError, reload);
    return () => {
      disposed = true;
      void api.cancelCatalog().catch(() => {});
      autoRefresh.dispose();
      stopObserving();
      scheduler.current = null;
    };
  }, [active, cache, onWatchError, refreshKey, scope]);

  useEffect(() => { scheduler.current?.setBlocked(blocked); }, [blocked]);

  const current = status?.scope === scope ? status : undefined;
  const error = current?.error ?? null;
  return {
    cache,
    catalog: catalog ?? EMPTY_CATALOG,
    loading: active && !!agent && !cache.peek(agent.id, filter) && !error,
    refreshing: active && (current?.refreshing ?? false),
    error,
    refresh: () => setRefreshKey(key => key + 1),
  };
}
