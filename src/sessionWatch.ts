import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { watchSessions } from "./api";
import type { AutoRefreshScheduler } from "./autoRefresh";

interface ChangeSignal { agent: string; generation: number; error?: string }
let generation = Date.now();
const nextGeneration = () => (generation = Math.max(generation + 1, Date.now()));

export function observeSessionChanges(
  agent: string,
  scheduler: AutoRefreshScheduler,
  onError: (error: string) => void,
  initialRefresh?: () => Promise<void>,
) {
  const native = isTauri();
  let disposed = false;
  let listening = false;
  let focused = document.hasFocus();
  // 优先使用原生窗口焦点，避免 WebView 内的焦点事件恢复后台监听。
  let nativeFocused: boolean | undefined;
  let active: boolean | undefined;
  let watchGeneration = 0;

  const update = async () => {
    const foreground = (nativeFocused ?? focused) && document.visibilityState === "visible";
    if (disposed || !listening || active === foreground) return;
    active = foreground;
    if (!foreground) scheduler.setActive(false);
    const ticket = watchGeneration = nextGeneration();
    try {
      if (native) await watchSessions(foreground ? agent : null, ticket);
    } catch (error) {
      if (!disposed && ticket === watchGeneration && foreground) onError(String(error));
    }
    if (disposed || ticket !== watchGeneration) return;
    scheduler.setActive(foreground);
    if (initialRefresh) {
      const refresh = initialRefresh;
      initialRefresh = undefined;
      await refresh();
    } else if (foreground) {
      scheduler.request("focus");
    }
  };

  const focus = () => { focused = true; void update(); };
  const blur = () => { focused = false; void update(); };
  const visibility = () => { focused = document.hasFocus(); void update(); };
  window.addEventListener("focus", focus);
  window.addEventListener("blur", blur);
  document.addEventListener("visibilitychange", visibility);

  const subscriptions = native ? Promise.allSettled([
    listen<ChangeSignal>("session-files-changed", ({ payload }) => {
      if (disposed || !active || payload.agent !== agent || payload.generation !== watchGeneration) return;
      if (payload.error) onError(payload.error);
      scheduler.request("change");
    }),
    listen<boolean>("session-window-focus", ({ payload }) => { nativeFocused = payload; void update(); }),
  ]) : Promise.resolve([]);
  void subscriptions.then(results => {
    if (disposed) return;
    for (const result of results) if (result.status === "rejected") onError(String(result.reason));
    listening = true;
    void update();
  });

  return () => {
    disposed = true;
    scheduler.setActive(false);
    window.removeEventListener("focus", focus);
    window.removeEventListener("blur", blur);
    document.removeEventListener("visibilitychange", visibility);
    if (native) void watchSessions(null, nextGeneration()).catch(() => {});
    void subscriptions.then(results => {
      for (const result of results) if (result.status === "fulfilled") result.value();
    }).catch(() => {});
  };
}
