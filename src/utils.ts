import type { SessionMeta } from "./types";

export function sessionKey(session: Pick<SessionMeta, "projectDir" | "sessionId">): string {
  return JSON.stringify([session.projectDir, session.sessionId]);
}

export function relativeTime(iso: string | null): string {
  if (!iso) return "—";
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return iso;
  const diff = Date.now() - t;
  const min = Math.floor(diff / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} 小时前`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day} 天前`;
  return new Date(t).toLocaleDateString("zh-CN");
}

export function fullTime(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString("zh-CN");
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function shortPath(p: string | null): string {
  if (!p) return "";
  const home = "/Users/";
  if (p.startsWith(home)) {
    const rest = p.slice(home.length);
    const idx = rest.indexOf("/");
    if (idx > 0) return "~" + rest.slice(idx);
  }
  return p;
}
