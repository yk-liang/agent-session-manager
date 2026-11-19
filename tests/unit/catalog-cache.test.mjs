import test from "node:test";
import assert from "node:assert/strict";
import { AgentCatalogCache } from "../../src/catalogCache.ts";

const generation = 1;

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
  await cache.load("qoder", false, generation);
  const detail = { ...row, userCount: 9, sizeBytes: 123 };
  row = { ...meta("same", "mode-new"), title: "Named desktop session", userCount: 99 };
  await cache.load("qoder", false, generation);
  const current = cache.withCurrentMetadata("qoder", false, detail);
  assert.equal(current.title,"Named desktop session");
  assert.equal(current.model,"mode-new");
  assert.equal(current.userCount,9);
  assert.equal(current.sizeBytes,123);
  assert.equal(current.projectDir,detail.projectDir);
  const unrelated={...detail,projectDir:"another-source"};
  assert.equal(cache.withCurrentMetadata("qoder",false,unrelated).title,"Continue");
});

test("cached metadata never clears a body read failure; only a successful body retry does", async () => {
  const row = {...meta("same", "mode-id"), realUserCount:1};
  const refresh = deferred(); let pending = false;
  const cache = new AgentCatalogCache(async () => pending ? refresh.promise : catalog(row));
  for (const agent of ["qoder", "claude"]) for (const empty of [false, true]) await cache.load(agent, empty, generation);
  pending = true;
  const load = cache.load("qoder", false, generation);
  cache.setReadError("qoder", row, "stage: open\nos_code: 24");
  refresh.resolve(catalog(row)); await load;
  for (const empty of [false, true]) assert.match(cache.peek("qoder", empty).sessions[0].readError, /os_code: 24/);
  assert.equal(cache.peek("claude", false).sessions[0].readError, undefined);
  await cache.refine("qoder", false, async () => ({...catalog(), warnings:["Directory cannot be read"]}));
  assert.equal(cache.peek("qoder", false).sessions[0].sessionId, "same");
  assert.equal(cache.peek("qoder", false).projects[0].displayPath, "/tmp/real");
  cache.setReadError("qoder", row, null);
  for (const empty of [false, true]) assert.equal(cache.peek("qoder", empty).sessions[0].readError, undefined);
  cache.setReadError("qoder", row, "failed again");
  cache.remove("qoder", [row]);
  await cache.refine("qoder", false, async () => ({...catalog(), warnings:["Directory unavailable"]}));
  assert.equal(cache.peek("qoder", false).sessions.length, 0, "deleted failure must not revive the cached card");
});

test("a coalesced refresh retains the matching snapshot when its revision is unchanged", async () => {
  let calls = 0;
  const held = deferred();
  const cache = new AgentCatalogCache(async (agent, empty, requestGeneration, revision) => {
    if (++calls === 1) return { ...catalog(meta(agent)), revision: 7 };
    assert.equal(revision, 7);
    return held.promise;
  });
  const snapshot = await cache.load("codex", false, generation);
  let changes = 0;
  const unsubscribe = cache.subscribe(() => { changes++; });
  const refresh = cache.load("codex", false, generation);
  assert.equal(cache.load("codex", false, generation), refresh);
  assert.equal(cache.peek("codex", false), snapshot);
  assert.equal(cache.peek("claude", false), undefined);
  assert.equal(cache.peek("codex", true), undefined);
  held.resolve(null);
  assert.equal(await refresh, snapshot);
  assert.equal(cache.peek("codex", false), snapshot);
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
  await cache.load("codex", false, generation);
  await assert.rejects(cache.load("codex", false, generation), /scan unavailable/);
  assert.equal(cache.peek("codex", false).sessions[0].sessionId, "1");
  await cache.load("codex", false, generation);
  assert.equal(cache.peek("codex", false).sessions[0].sessionId, "3");
});

test("late scans cannot revive deleted sessions or touch another Agent", async () => {
  const scan = deferred();
  let hold = false;
  const row = meta("same", "mode-id");
  const cache = new AgentCatalogCache(async () => hold ? scan.promise : catalog(row));
  for (const agent of ["qoder", "claude"]) for (const empty of [false, true]) await cache.load(agent, empty, generation);
  hold = true;
  const stale = cache.load("qoder", false, generation);
  cache.remove("qoder", [row]);
  scan.resolve(catalog(row));
  await stale;
  assert.equal(cache.peek("qoder", false).sessions.length, 0);
  assert.equal(cache.peek("qoder", true).projects.length, 0);
  assert.equal(cache.peek("claude", false).sessions.length, 1);
});

test("background statistics cannot replace a newer refresh or revive deleted sessions", async () => {
  let rows = [meta("first")];
  const cache = new AgentCatalogCache(async () => catalog(...rows));
  await cache.load("codex", false, generation);
  const old = deferred(); const refine = cache.refine("codex", false, () => old.promise);
  rows = [meta("new")]; await cache.load("codex", false, generation);
  old.resolve(catalog(meta("first")));
  assert.equal(await refine, undefined);
  const deleted = deferred(); const pending = cache.refine("codex", false, () => deleted.promise);
  cache.remove("codex", rows); deleted.resolve(catalog(...rows));
  assert.equal(await pending, undefined);
  assert.equal(cache.peek("codex", false).sessions.length, 0);
});
