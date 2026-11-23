import type { SessionMeta } from "./types";
import { sessionKey } from "./utils.ts";

export interface SessionListRow {
  session: SessionMeta;
  key: string;
  depth: number;
  childCount: number;
  guides: boolean[];
}

export function sessionTree(sessions: SessionMeta[]) {
  const byKey = new Map(sessions.map(session => [sessionKey(session), session]));
  const parents = new Map<string, string>();
  for (const session of sessions) {
    const relation = session.subagent;
    if (!relation?.parent || relation.orphan) continue;
    const parent = sessionKey(relation.parent);
    if (byKey.has(parent)) parents.set(sessionKey(session), parent);
  }

  const owners = new Map<string, string>();
  const children = new Map<string, SessionMeta[]>();
  for (const session of sessions) {
    const key = sessionKey(session);
    if (!parents.has(key)) continue;
    let owner = key;
    const path = new Set<string>();
    while (parents.has(owner) && !owners.has(owner)) {
      if (path.has(owner)) {
        let member = owner;
        do {
          const parent = parents.get(member)!;
          parents.delete(member);
          member = parent;
        } while (member !== owner);
        break;
      }
      path.add(owner);
      owner = parents.get(owner)!;
    }
    for (const member of path) {
      if (parents.has(member)) owners.set(member, owners.get(owner) ?? owner);
    }
  }
  for (const session of sessions) {
    const parent = parents.get(sessionKey(session));
    if (!parent) continue;
    const siblings = children.get(parent);
    if (siblings) siblings.push(session);
    else children.set(parent, [session]);
  }

  const ancestors = (key: string, scope?: ReadonlySet<string>) => {
    const path: string[] = [];
    let parent = parents.get(key);
    while (parent && (!scope || scope.has(parent))) {
      path.push(parent);
      parent = parents.get(parent);
    }
    return path;
  };

  const rows = (scope: SessionMeta[], expanded: ReadonlySet<string>, flat: boolean): SessionListRow[] => {
    if (flat) return scope.map(session => ({ session, key: sessionKey(session), depth: 0, childCount: 0, guides: [] }));
    const keys = new Set(scope.map(sessionKey));
    const roots = scope.filter(session => {
      const parent = parents.get(sessionKey(session));
      return !parent || !keys.has(parent);
    });
    const result: SessionListRow[] = [];
    const pending = roots.map(session => ({ session, depth: 0, guides: [] as boolean[] })).reverse();
    while (pending.length) {
      const row = pending.pop()!;
      const key = sessionKey(row.session);
      const group = children.get(key)?.filter(session => keys.has(sessionKey(session))) ?? [];
      result.push({ ...row, key, childCount: group.length });
      if (!expanded.has(key)) continue;
      for (let index = group.length - 1; index >= 0; index--) {
        const guides = [...row.guides.slice(0, 2), index + 1 < group.length];
        pending.push({ session: group[index], depth: row.depth + 1, guides });
      }
    }
    return result;
  };

  return {
    byKey, ancestors, rows,
    parent: (session: SessionMeta) => byKey.get(parents.get(sessionKey(session)) ?? ""),
    owner: (session: SessionMeta) => byKey.get(owners.get(sessionKey(session)) ?? "") ?? session,
  };
}
