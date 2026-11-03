import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "./api";
import type { AgentInfo } from "./types";

export interface UsageModelRef { agent: string; model: string }
export type UsageModelLabel = (agent: string, model: string, fallback?: string) => string;
const modelKey = (agent: string, model: string) => JSON.stringify([agent, model]);

/** 复用会话页的名称接口；只补显示名，不改统计身份，也不让 CLI 阻塞用量读取。 */
export function useUsageModelNames(agents: AgentInfo[], rows: UsageModelRef[], revision: number) {
  const [events, setEvents] = useState<UsageModelRef[]>([]);
  const [names, setNames] = useState(() => new Map<string, string>());
  const pending = useRef(new Map<string, Promise<Record<string, string>>>());
  const supported = new Set(agents.filter(a => a.supportsModelNames || a.id === "qoder").map(a => a.id));
  const grouped = new Map<string, Set<string>>();
  // 失败请求可能不在用量汇总中，因此也收集已加载请求页的模型。
  for (const { agent, model } of [...rows, ...events]) {
    if (!supported.has(agent) || !model || model === "unknown") continue;
    if (!grouped.has(agent)) grouped.set(agent, new Set());
    grouped.get(agent)!.add(model);
  }
  const queryKey = JSON.stringify([...grouped].sort(([a], [b]) => a.localeCompare(b)).map(([agent, ids]) => [agent, [...ids].sort()]));
  useEffect(() => {
    let disposed = false;
    const groups: [string, string[]][] = JSON.parse(queryKey);
    for (const [agent, models] of groups) void (async () => {
      // 与会话页保持相同的 256 个 ID 上限；同步或翻页期间复用尚未完成的补全。
      for (let offset = 0; offset < models.length && !disposed; offset += 256) {
        const batch = models.slice(offset, offset + 256);
        const key = JSON.stringify([agent, batch]);
        let request = pending.current.get(key);
        if (!request) {
          request = api.resolveModelNames(agent, batch).finally(() => { pending.current.delete(key); });
          pending.current.set(key, request);
        }
        try {
          const resolved = await request;
          if (disposed) return;
          setNames(previous => {
            const next = new Map(previous);
            for (const [id, name] of Object.entries(resolved ?? {})) {
              if (name.trim()) next.set(modelKey(agent, id), name);
            }
            return next;
          });
        } catch {
          // 离线或名称暂不可查时保留原标签，下一轮用量同步后重试。
        }
      }
    })();
    return () => { disposed = true; };
  }, [queryKey, revision]);

  // Agent 也是身份的一部分；不同来源即使记录相同 ID，也不能互相借用名称。
  const modelLabel: UsageModelLabel = useCallback((agent, model, fallback) =>
    names.get(modelKey(agent, model)) ?? (fallback || model), [names]);
  return { modelLabel, onEventModels: setEvents };
}
