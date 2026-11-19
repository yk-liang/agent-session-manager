import test from "node:test";
import assert from "node:assert/strict";
import { AgentCatalogCache, DEFAULT_CATALOG_FILTER as active } from "../../src/catalogCache.ts";

const generation = 1;
const allActive = { ...active, includeEmpty: true };
const withArchived = { ...active, includeArchived: true };

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const meta = (id, model = null) => ({ sessionId: id, projectDir: "real-dir", title: id,
  model, lastTs: null });
const catalog = (...sessions) => ({ sessions, warnings: [], projects: sessions.length ?
  [{ dirName: "real-dir", displayPath: "/tmp/real", sessionCount: sessions.length, lastActivity: null }] : [] });

test("desktop rename and model selection update an open detail without replacing its body statistics", async () => {
  let row = {...meta("same", "mode-old"), title:"Continue"};
  const cache = new AgentCatalogCache(async () => catalog(row));
  await cache.load("qoder", active, generation);
  const detail = { ...row, userCount: 9, sizeBytes: 123 };
  row = { ...meta("same", "mode-new"), title: "Named desktop session", userCount: 99 };
  await cache.load("qoder", active, generation);
  const current = cache.withCurrentMetadata("qoder", active, detail);
  assert.equal(current.title,"Named desktop session");
  assert.equal(current.model,"mode-new");
  assert.equal(current.userCount,9);
  assert.equal(current.sizeBytes,123);
  assert.equal(current.projectDir,detail.projectDir);
  const unrelated={...detail,projectDir:"another-source"};
  assert.equal(cache.withCurrentMetadata("qoder", active,unrelated).title,"Continue");
});

test("cached metadata never clears a body read failure; only a successful body retry does", async () => {
  const row = {...meta("same", "mode-id"), realUserCount:1};
  const refresh = deferred(); let pending = false;
  const cache = new AgentCatalogCache(async () => pending ? refresh.promise : catalog(row));
  for (const agent of ["qoder", "claude"]) for (const filter of [active, allActive]) await cache.load(agent, filter, generation);
  pending = true;
  const load = cache.load("qoder", active, generation);
  cache.setReadError("qoder", row, "stage: open\nos_code: 24");
  refresh.resolve(catalog(row)); await load;
  for (const filter of [active, allActive]) assert.match(cache.peek("qoder", filter).sessions[0].readError, /os_code: 24/);
  assert.equal(cache.peek("claude", active).sessions[0].readError, undefined);
  await cache.refine("qoder", active, async () => ({...catalog(), warnings:["Directory cannot be read"]}));
  assert.equal(cache.peek("qoder", active).sessions[0].sessionId, "same");
  assert.equal(cache.peek("qoder", active).projects[0].displayPath, "/tmp/real");
  cache.setReadError("qoder", row, null);
  for (const filter of [active, allActive]) assert.equal(cache.peek("qoder", filter).sessions[0].readError, undefined);
  cache.setReadError("qoder", row, "failed again");
  cache.remove("qoder", [row]);
  await cache.refine("qoder", active, async () => ({...catalog(), warnings:["Directory unavailable"]}));
  assert.equal(cache.peek("qoder", active).sessions.length, 0, "deleted failure must not revive the cached card");
});

test("a coalesced refresh retains the matching snapshot when its revision is unchanged", async () => {
  let calls = 0;
  const held = deferred();
  const cache = new AgentCatalogCache(async (agent, filter, requestGeneration, revision) => {
    if (++calls === 1) return { ...catalog(meta(agent)), revision: 7 };
    assert.equal(revision, 7);
    return held.promise;
  });
  const snapshot = await cache.load("codex", active, generation);
  let changes = 0;
  const unsubscribe = cache.subscribe(() => { changes++; });
  const refresh = cache.load("codex", active, generation);
  assert.equal(cache.load("codex", active, generation), refresh);
  assert.equal(cache.peek("codex", active), snapshot);
  assert.equal(cache.peek("claude", active), undefined);
  assert.equal(cache.peek("codex", allActive), undefined);
  held.resolve(null);
  assert.equal(await refresh, snapshot);
  assert.equal(cache.peek("codex", active), snapshot);
  assert.equal(changes, 0);
  assert.equal(calls, 2);
  unsubscribe();
});

test("refresh failures preserve the previous list and allow a subsequent retry", async () => {
  let calls = 0;
  const cache = new AgentCatalogCache(async () => {
    if (++calls === 2) throw Error("scan unavailable");
    return catalog(meta(String(calls)));
  });
  await cache.load("codex", active, generation);
  await assert.rejects(cache.load("codex", active, generation), /scan unavailable/);
  assert.equal(cache.peek("codex", active).sessions[0].sessionId, "1");
  await cache.load("codex", active, generation);
  assert.equal(cache.peek("codex", active).sessions[0].sessionId, "3");
});

test("late scans cannot revive deleted sessions or touch another Agent", async () => {
  const scan = deferred();
  const count = deferred();
  let hold = false;
  const row = meta("same", "mode-id");
  const cache = new AgentCatalogCache(async () => hold ? scan.promise : catalog(row));
  for (const agent of ["qoder", "claude"]) for (const filter of [active, allActive]) await cache.load(agent, filter, generation);
  const refreshCount = () => cache.refreshCount("qoder", active, () => count.promise);
  const staleCount = refreshCount();
  assert.equal(refreshCount(), staleCount);
  hold = true;
  const stale = cache.load("qoder", active, generation);
  cache.remove("qoder", [row]);
  scan.resolve(catalog(row));
  count.resolve(1);
  await Promise.all([stale, staleCount]);
  assert.equal(cache.peek("qoder", active).sessions.length, 0);
  assert.equal(cache.peek("qoder", allActive).projects.length, 0);
  assert.equal(cache.peek("claude", active).sessions.length, 1);
  assert.equal(cache.count("qoder", active), 0);
});

test("background statistics cannot replace a newer refresh or revive deleted sessions", async () => {
  let row = { ...meta("first"), realUserCount: 1 };
  const cache = new AgentCatalogCache(async (_agent, filter) =>
    row.archived && !filter.includeArchived ? catalog() : catalog(row));
  await cache.load("codex", active, generation);
  const old = deferred(); const refine = cache.refine("codex", active, () => old.promise);
  row = { ...row, sessionId: "new" };
  await cache.load("codex", active, generation);
  old.resolve(catalog(meta("first")));
  assert.equal(await refine, undefined);

  await cache.load("codex", withArchived, generation);
  const stale = deferred(); const pending = cache.refine("codex", active, () => stale.promise);
  cache.setArchived("codex", row, true);
  stale.resolve(catalog(row));
  assert.equal(await pending, undefined);
  assert.equal(cache.peek("codex", active).sessions.length, 0);
  assert.equal(cache.peek("codex", withArchived).sessions[0].archived, true);

  cache.setArchived("codex", { ...row, archived: true }, false);
  assert.equal(cache.peek("codex", active).sessions[0].archived, false);
  assert.equal(cache.peek("codex", active).projects[0].sessionCount, 1);
  cache.remove("codex", [row]);
  assert.equal(cache.peek("codex", withArchived).sessions.length, 0);
});
