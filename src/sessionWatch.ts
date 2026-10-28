import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { watchSessions } from "./api";
import type { AutoRefreshScheduler } from "./autoRefresh";

interface ChangeSignal { agent: string; generation: number; error?: string }
let generation = Date.now();
const nextGeneration = () => (generation = Math.max(generation + 1, Date.now()));

/** 与当前目录上下文绑定；恢复前台时检查一次，后台关闭文件事件流。 */
export function observeSessionChanges(agent: string, scheduler: AutoRefreshScheduler, onError: (error: string) => void) {
  let disposed = false;
  let focused = document.hasFocus();
  // 原生窗口焦点优先，WebView 内部的焦点变化不能把后台窗口重新判为前台。
  let nativeFocused: boolean | undefined;
  let active: boolean | undefined;
  let currentGeneration = 0;
  const native = isTauri();
  const update = (force = false) => {
    if (disposed) return;
    const foreground = (nativeFocused ?? focused) && document.visibilityState === "visible";
    if (!force && active === foreground) return;
    active = foreground;
    scheduler.setActive(foreground);
    currentGeneration = nextGeneration();
    const requestGeneration = currentGeneration;
    if (native) void watchSessions(foreground ? agent : null, requestGeneration).then(() => {
      // 注册监听的间隙可能已有新日志；补一次轻量检查，避免丢掉新建目录事件。
      if (!disposed && active && currentGeneration === requestGeneration) scheduler.request("focus");
    }).catch((error) => { if (!disposed && active && currentGeneration === requestGeneration) onError(String(error)); });
  };
  const focus = () => { focused = true; update(); };
  const blur = () => { focused = false; update(); };
  const visibility = () => { focused = document.hasFocus(); update(); };
  window.addEventListener("focus", focus);
  window.addEventListener("blur", blur);
  document.addEventListener("visibilitychange", visibility);
  const subscriptions = native ? Promise.allSettled([
    listen<ChangeSignal>("session-files-changed", ({ payload }) => {
      if (disposed || !active || payload.agent !== agent || payload.generation !== currentGeneration) return;
      if (payload.error) onError(payload.error);
      scheduler.request("change");
    }),
    listen<boolean>("session-window-focus", ({ payload }) => { nativeFocused = payload; update(); }),
  ]).then((results) => {
    const unlisten = [];
    for (const result of results) {
      if (result.status === "fulfilled") unlisten.push(result.value);
      else if (!disposed) onError(String(result.reason));
    }
    return unlisten;
  }) : Promise.resolve([]);
  void subscriptions.then(() => {
    if (!disposed) update(true);
  }).catch((error) => { if (!disposed) onError(String(error)); });
  return () => {
    disposed = true;
    scheduler.setActive(false);
    window.removeEventListener("focus", focus);
    window.removeEventListener("blur", blur);
    document.removeEventListener("visibilitychange", visibility);
    if (native) void watchSessions(null, nextGeneration()).catch(() => {});
    void subscriptions.then((unlisten) => Promise.allSettled(unlisten.map((off) => off()))).catch(() => {});
  };
}
