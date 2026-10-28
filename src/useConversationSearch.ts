import { useEffect, useMemo, useState } from "react";
import * as api from "./api";
import { sessionKey } from "./utils";
import type { ConversationSearchHit, SessionMeta } from "./types";

export function useConversationSearch(
  agent: string | null,
  query: string,
  sessions: SessionMeta[],
  enabled: boolean,
) {
  const [retryKey, setRetryKey] = useState(0);
  // 状态绑定到不可变的查询上下文，旧 Agent、项目或关键词的结果不参与当前列表。
  const context = useMemo(
    () => ({ agent, query: query.trim(), sessions, enabled, retryKey }),
    [agent, query, sessions, enabled, retryKey],
  );
  const [state, setState] = useState<{
    context: typeof context;
    hits: ConversationSearchHit[];
    pending: boolean;
    skipped: number;
    error: string | null;
  } | null>(null);

  useEffect(() => {
    if (!enabled || !agent || !context.query || sessions.length === 0) return;
    let cancelled = false;
    let started = false;
    const timer = window.setTimeout(() => {
      started = true;
      void api
        .searchConversations(agent, context.query, sessions)
        .then((result) => {
          if (!cancelled)
            setState({
              context,
              hits: result.cancelled ? [] : result.hits,
              pending: false,
              skipped: result.skipped,
              error: result.cancelled ? "搜索已取消，请重试" : null,
            });
        })
        .catch((error) => {
          if (!cancelled)
            setState({
              context,
              hits: [],
              pending: false,
              skipped: 0,
              error: String(error),
            });
        });
    }, 250);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      if (started) void api.cancelConversationSearch().catch(() => {});
    };
  }, [context, enabled, agent, sessions]);

  const current = state?.context === context ? state : null;
  const hits = useMemo(
    () => new Map((current?.hits ?? []).map((hit) => [sessionKey(hit), hit])),
    [current],
  );
  return {
    retry: () => setRetryKey(key => key + 1),
    hits,
    pending:
      enabled && !!agent && !!context.query && sessions.length > 0 && !current,
    skipped: current?.skipped ?? 0,
    error: current?.error ?? null,
  };
}
