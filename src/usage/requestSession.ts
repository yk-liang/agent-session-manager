import type { Catalog } from "../types";
import { displaySessionId } from "../utils";
import type { RequestEvent } from "./types";

export function requestSessionIds({ session, request }: RequestEvent) {
  return [...new Set([request.subagentId, session, request.parentSessionId].map(id => id?.trim()).filter((id): id is string => !!id))];
}

const absolutePath = (path: string | null | undefined) => path?.startsWith("/") ? path.replace(/\/+$/, "") || "/" : undefined;
const within = (path: string, root: string | undefined) => !!root && (path === root || path.startsWith(`${root}/`));

export function findRequestSession(catalog: Catalog, event: RequestEvent) {
  const path = absolutePath(event.request.absoluteProjectPath ?? event.request.project);
  const projects = new Map(catalog.projects.map(project => [project.dirName, absolutePath(project.displayPath)]));
  const subagentId = event.request.subagentId?.trim();
  for (const id of requestSessionIds(event)) {
    // 旧 Pi 用量记录可能以日志路径作为会话 ID。
    const [source, filename] = event.agent === "pi" && id.startsWith("/") && id.endsWith(".jsonl") ? id.split("/").slice(-2) : [];
    const sessionId = filename ? filename.slice(0, -6).split("_").pop() : id;
    const candidates = catalog.sessions.filter(session => session.hasTranscript && (!source || session.projectDir === source));
    const children = id === subagentId ? candidates.filter(session => session.subagent && displaySessionId(session) === sessionId) : [];
    const matches = children.length ? children : candidates.filter(session => session.sessionId === sessionId);
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
