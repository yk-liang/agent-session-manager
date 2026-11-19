import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import * as api from "./api";
import { AutoRefreshScheduler } from "./autoRefresh";
import { AgentCatalogCache, EMPTY_CATALOG } from "./catalogCache";
import { observeSessionChanges } from "./sessionWatch";
import type { AgentInfo } from "./types";

export function useSessionCatalog(
  agent: AgentInfo | undefined,
  includeEmpty: boolean,
  active: boolean,
  blocked: boolean,
  onWatchError: (error: string) => void,
) {
  const [cache] = useState(() => new AgentCatalogCache(api.catalog, api.resolveModelNames));
  const [refreshKey, setRefreshKey] = useState(0);
  const scope = useMemo(() => ({ agent, includeEmpty }), [agent, includeEmpty]);
  const catalog = useSyncExternalStore(cache.subscribe, () => active && agent ? cache.visible(agent.id, includeEmpty) : undefined);
  const [status, setStatus] = useState<{ scope: typeof scope; refreshing: boolean; error: string | null }>();
  const scheduler = useRef<AutoRefreshScheduler | null>(null);
  const blockedRef = useRef(blocked);
  blockedRef.current = blocked;

  useEffect(() => {
    if (!scope.agent || !active) return;
    const { id, supportsModelNames } = scope.agent;
    const { includeEmpty } = scope;
    let disposed = false;
    const autoRefresh = new AutoRefreshScheduler(reload);
    scheduler.current = autoRefresh;
    autoRefresh.setBlocked(blockedRef.current);

    async function reload() {
      const generation = api.nextCatalogGeneration();
      autoRefresh.setBusy(true);
      setStatus({ scope, refreshing: true, error: null });
      try {
        const catalog = await cache.load(id, includeEmpty, generation);
        if (disposed) return;
        setStatus({ scope, refreshing: false, error: null });
        if (supportsModelNames) void cache.enrich(id, includeEmpty).catch(() => {});
        if (catalog.sessions.some(session => session.statsPending)) {
          await cache.refine(id, includeEmpty, async revision => {
            const refined = await api.refineCatalog(id, includeEmpty, generation, revision);
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
    loading: active && !!agent && !cache.peek(agent.id, includeEmpty) && !error,
    refreshing: active && (current?.refreshing ?? false),
    error,
    refresh: () => setRefreshKey(key => key + 1),
  };
}
