import type { Catalog } from "../types";
import type { RequestEvent } from "./types";

export function requestSessionIds({ session, request }: RequestEvent) {
  return [...new Set([request.subagentId, session, request.parentSessionId].map(id => id?.trim()).filter((id): id is string => !!id))];
}

const absolutePath = (path: string | null | undefined) => path?.startsWith("/") ? path.replace(/\/+$/, "") || "/" : undefined;
const within = (path: string, root: string | undefined) => !!root && (path === root || path.startsWith(`${root}/`));

export function findRequestSession(catalog: Catalog, event: RequestEvent) {
  const path = absolutePath(event.request.absoluteProjectPath ?? event.request.project);
  const projects = new Map(catalog.projects.map(project => [project.dirName, absolutePath(project.displayPath)]));
  for (const id of requestSessionIds(event)) {
    // 旧 Pi 用量记录可能以日志路径作为会话 ID。
    const [source, filename] = event.agent === "pi" && id.startsWith("/") && id.endsWith(".jsonl") ? id.split("/").slice(-2) : [];
    const sessionId = filename ? filename.slice(0, -6).split("_").pop() : id;
    const matches = catalog.sessions.filter(session => session.sessionId === sessionId && session.hasTranscript
      && (!source || session.projectDir === source));
    // Codex 按线程 ID 读取；请求的工作目录可能随回合变化。
    if (matches.length === 1 && (!path || source || event.agent === "codex")) return matches[0];
    if (!path || matches.length === 0) continue;
    const byCwd = matches.filter(session => absolutePath(session.cwd) === path);
    if (byCwd.length === 1) return byCwd[0];
    const byProject = (byCwd.length ? byCwd : matches).filter(session =>
      within(path, absolutePath(session.cwd)) || within(path, projects.get(session.projectDir)));
    if (byProject.length === 1) return byProject[0];
    if (matches.length === 1 && !absolutePath(matches[0].cwd) && !projects.get(matches[0].projectDir)) return matches[0];
  }
}
