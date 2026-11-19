import type { SessionMeta } from "./types";

export function sessionKey(session: Pick<SessionMeta, "projectDir" | "sessionId">): string {
  return JSON.stringify([session.projectDir, session.sessionId]);
}

export function relativeTime(iso: string | null): string {
  if (!iso) return "—";
  const timestamp = new Date(iso).getTime();
  if (Number.isNaN(timestamp)) return iso;
  const elapsedMs = Date.now() - timestamp;
  const minutes = Math.floor(elapsedMs / 60000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  const day = Math.floor(hours / 24);
  if (day < 30) return `${day} 天前`;
  return new Date(timestamp).toLocaleDateString("zh-CN");
}

export function formatLocalDateTime(iso: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString("zh-CN");
}

export function formatMessageTimestamp(timestamp: string): string | undefined {
  const date = new Date(timestamp);
  if (!Number.isFinite(date.getTime())) return;
  const pad = (value: number) => String(value).padStart(2, "0");
  const year = String(date.getFullYear()).padStart(4, "0");
  return `${year}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function shortPath(path: string | null): string {
  if (!path) return "";
  const usersDir = "/Users/";
  if (path.startsWith(usersDir)) {
    const userPath = path.slice(usersDir.length);
    const separator = userPath.indexOf("/");
    if (separator > 0) return "~" + userPath.slice(separator);
  }
  return path;
}

export function projectName(path: string | null): string {
  return path?.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path || "项目未记录";
}
