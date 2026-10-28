import test from "node:test";
import assert from "node:assert/strict";
import { AgentCatalogCache } from "../../src/catalogCache.ts";

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const meta = (id, model = null) => ({ sessionId: id, projectDir: "real-dir", title: id,
  model, modelName: null, lastTs: null });
const catalog = (...sessions) => ({ sessions, warnings: [], projects: sessions.length ?
  [{ dirName: "real-dir", displayPath: "/tmp/real", sessionCount: sessions.length, lastActivity: null }] : [] });

test("switching back exposes the matching snapshot while a coalesced refresh is pending", async () => {
  let calls = 0;
  const held = deferred();
  const cache = new AgentCatalogCache(async (agent, empty) => {
    calls++;
    return calls === 1 ? catalog(meta(agent)) : held.promise;
  }, async () => ({}));
  await cache.load("codex", false);
  const refresh = cache.load("codex", false);
  assert.equal(cache.load("codex", false), refresh);
  assert.equal(cache.peek("codex", false).sessions[0].sessionId, "codex");
  assert.equal(cache.peek("claude", false), undefined);
  assert.equal(cache.peek("codex", true), undefined);
  assert.equal(calls, 2);
  held.resolve(catalog(meta("new")));
  await refresh;
  assert.equal(cache.peek("codex", false).sessions[0].sessionId, "new");
});

test("refresh failures preserve the previous list and allow a subsequent retry", async () => {
  let calls = 0;
  const cache = new AgentCatalogCache(async () => {
    if (++calls === 2) throw Error("scan unavailable");
    return catalog(meta(String(calls)));
  }, async () => ({}));
  await cache.load("codex", false);
  await assert.rejects(cache.load("codex", false), /scan unavailable/);
  assert.equal(cache.peek("codex", false).sessions[0].sessionId, "1");
  await cache.load("codex", false);
  assert.equal(cache.peek("codex", false).sessions[0].sessionId, "3");
});

test("names resolve independently, merge into both filters and survive a raw metadata refresh", async () => {
  const names = deferred(); let nameCalls = 0;
  const cache = new AgentCatalogCache(async () => catalog(meta("same", "mode-id")),
    async () => { nameCalls++; return names.promise; });
  await cache.load("qoder", false);
  await cache.load("qoder", true);
  await cache.load("codex", false);
  const first = cache.enrich("qoder", false);
  const second = cache.enrich("qoder", true);
  assert.equal(cache.peek("qoder", false).sessions.length, 1);
  assert.equal(nameCalls, 1);
  names.resolve({ "mode-id": "Qwen example" });
  await Promise.all([first, second]);
  assert.equal(cache.peek("qoder", true).sessions[0].modelName, "Qwen example");
  assert.equal(cache.peek("codex", false).sessions[0].modelName, null);
  await cache.load("qoder", false);
  assert.equal(cache.peek("qoder", false).sessions[0].modelName, "Qwen example");
  assert.equal(cache.peek("qoder", false).sessions[0].model, "mode-id");
});

test("late scans and model lookups cannot revive deleted sessions or touch another Agent", async () => {
  const scan = deferred(), names = deferred(); let hold = false;
  const row = meta("same", "mode-id");
  const cache = new AgentCatalogCache(async () => hold ? scan.promise : catalog(row), async () => names.promise);
  for (const agent of ["qoder", "claude"]) for (const empty of [false, true]) await cache.load(agent, empty);
  const naming = cache.enrich("qoder", false);
  hold = true;
  const stale = cache.load("qoder", false);
  cache.remove("qoder", [row]);
  scan.resolve(catalog(row));
  names.resolve({ "mode-id": "Friendly name" });
  await Promise.all([stale, naming]);
  assert.equal(cache.peek("qoder", false).sessions.length, 0);
  assert.equal(cache.peek("qoder", true).projects.length, 0);
  assert.equal(cache.peek("claude", false).sessions.length, 1);
});

test("late name completion enriches the newest list instead of replacing it", async () => {
  const names = deferred(); let rows = [meta("first", "mode-id")];
  const cache = new AgentCatalogCache(async () => catalog(...rows), async () => names.promise);
  await cache.load("qoder", false);
  const naming = cache.enrich("qoder", false);
  rows = [meta("second", "mode-id")];
  await cache.load("qoder", false);
  names.resolve({ "mode-id": "Readable name" });
  await naming;
  assert.deepEqual(cache.peek("qoder", false).sessions.map(s => s.sessionId), ["second"]);
  assert.equal(cache.peek("qoder", false).sessions[0].modelName, "Readable name");
});

test("capable sources refresh known names without treating model IDs as object properties", async () => {
  let name = "Old name";
  const cache = new AgentCatalogCache(async () => catalog(meta("a", "constructor")), async () => ({ constructor: name }));
  await cache.load("qoder", false);
  assert.equal(cache.peek("qoder", false).sessions[0].modelName, null);
  await cache.enrich("qoder", false, true);
  assert.equal(cache.peek("qoder", false).sessions[0].modelName, "Old name");
  name = "Updated name";
  await cache.enrich("qoder", false, true);
  assert.equal(cache.peek("qoder", false).sessions[0].modelName, "Updated name");
});

test("background statistics cannot replace a newer refresh or revive deleted sessions", async () => {
  let rows = [meta("first")];
  const cache = new AgentCatalogCache(async () => catalog(...rows), async () => ({}));
  await cache.load("codex", false);
  const old = deferred(); const refine = cache.refine("codex", false, () => old.promise);
  rows = [meta("new")]; await cache.load("codex", false);
  old.resolve(catalog(meta("first")));
  assert.equal(await refine, undefined);
  const deleted = deferred(); const pending = cache.refine("codex", false, () => deleted.promise);
  cache.remove("codex", rows); deleted.resolve(catalog(...rows));
  assert.equal(await pending, undefined);
  assert.equal(cache.peek("codex", false).sessions.length, 0);
});
