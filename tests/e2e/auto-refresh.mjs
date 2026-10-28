import { engine, baseURL, artifactDir } from "../helpers/browser.mjs";
import { chromium, webkit } from "playwright";
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import path from "node:path";

const browser = engine === "webkit" ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: "chrome", headless: true });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
const errors = []; page.on("pageerror", error => errors.push(error.message));
let checks = 0; const pass = label => console.log(`PASS ${++checks}: ${label}`);
try {
  await page.addInitScript(() => {
    const row = (id, title) => ({ sessionId: id, title, projectDir: "real-source", cwd: "/tmp/real-source", model: null,
      modelName: null, firstTs: null, lastTs: null, userCount: 1, assistantCount: 1, realUserCount: 1, assistantTextCount: 1,
      sizeBytes: 100, hasTranscript: true });
    const state = { rows: { qoder: [row("q1", "Existing Qoder session")], codex: [row("c1", "Existing Codex session")] },
      calls: [], catalogs: [], watch: { agent: null, generation: 0 }, callbacks: {}, listeners: {}, next: 0 };
    state.emit = (event, payload) => {
      for (const [id, listener] of Object.entries(state.listeners)) if (listener.event === event) {
        state.callbacks[listener.handler]?.({ event, id: Number(id), payload });
      }
    };
    state.changed = (agent = state.watch.agent, generation = state.watch.generation) => state.emit("session-files-changed", { agent, generation });
    state.add = (agent, id, title) => state.rows[agent].push(row(id, title));
    window.__AUTO_TEST__ = state;
    window.isTauri = true;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = { transformCallback: callback => { const id = ++state.next; state.callbacks[id] = callback; return id; },
      invoke: async (command, args = {}) => {
        state.calls.push({ command, args: structuredClone(args) });
        if (command === "plugin:event|listen") { const id = ++state.next; state.listeners[id] = args; return id; }
        if (command === "plugin:event|unlisten") { delete state.listeners[args.eventId]; return null; }
        if (command === "set_config_guard") return null;
        if (command === "list_agents") return [
          { id: "qoder", displayName: "Qoder CLI", installed: true, supportsFork: false, deletePolicy: "trash" },
          { id: "codex", displayName: "Codex", installed: true, supportsFork: false, deletePolicy: "trash" }];
        if (command === "watch_sessions") {
          if (args.generation >= state.watch.generation) state.watch = structuredClone(args);
          return null;
        }
        if (command === "catalog") {
          state.catalogs.push({ agent: args.agent, at: performance.now() });
          const sessions = structuredClone(state.rows[args.agent]);
          return { sessions, projects: [{ dirName: "real-source", displayPath: "/tmp/real-source", sessionCount: sessions.length, lastActivity: null }], warnings: [] };
        }
        if (command === "search_conversations") return { hits: [], skipped: 0, cancelled: false };
        if (command === "get_config_workspace") return { agent: args.agent, revision: "fixture", files: [], profiles: [],
          matchingProfiles: [], overview: { model: null, provider: null, endpoint: null, mcpServers: [] }, backups: [], warnings: [] };
        if (command === "read_session") return { meta: state.rows[args.agent].find(s => s.sessionId === args.sessionId),
          events: [{ type: "assistant", uuid: "stable", message: { content: "Keep the open conversation stable" } }] };
        return null;
      } };
  });
  await page.goto(baseURL);
  await page.locator(".session-card").first().waitFor();
  await page.evaluate(() => window.__AUTO_TEST__.emit("session-window-focus", true));
  await page.waitForFunction(() => window.__AUTO_TEST__.watch.agent === "qoder");
  await page.waitForTimeout(350);
  const count = () => page.evaluate(() => window.__AUTO_TEST__.catalogs.length);
  let before = await count();
  await page.evaluate(() => { window.__AUTO_TEST__.add("qoder", "q2", "New CLI session"); window.__AUTO_TEST__.changed(); });
  await page.locator(".session-card", { hasText: "New CLI session" }).waitFor({ timeout: 8000 });
  assert.equal(await count(), before + 1);
  pass("a new CLI session appears from a file event without a manual refresh");

  await page.locator(".session-card", { hasText: "Existing Qoder session" }).click();
  await page.locator(".detail-title").waitFor();
  before = await count();
  await page.evaluate(() => {
    const state = window.__AUTO_TEST__; state.rows.qoder[0].realUserCount = 8; state.rows.qoder[0].lastTs = "2026-10-19T00:00:00Z";
    for (let i = 0; i < 1000; i++) state.changed();
  });
  await page.waitForFunction(() => document.querySelector(".session-card")?.textContent.includes("9 条对话"), undefined, { timeout: 8000 });
  assert.equal(await count(), before + 1);
  assert.equal(await page.locator(".detail-title").textContent(), "Existing Qoder session");
  pass("continuing a session updates its count/time and a burst of 1000 events causes one scan without replacing the open detail");

  before = await count();
  await page.evaluate(() => window.__AUTO_TEST__.emit("session-window-focus", false));
  await page.waitForFunction(() => window.__AUTO_TEST__.watch.agent === null);
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForTimeout(100);
  assert.equal(await page.evaluate(() => window.__AUTO_TEST__.watch.agent), null);
  await page.evaluate(() => { const s = window.__AUTO_TEST__; s.add("qoder", "q3", "Created while background"); s.changed("qoder", s.watch.generation - 1); });
  await page.waitForTimeout(1500);
  assert.equal(await count(), before);
  assert.equal(await page.locator(".session-card").count(), 2);
  await page.evaluate(() => window.__AUTO_TEST__.emit("session-window-focus", true));
  await page.locator(".session-card", { hasText: "Created while background" }).waitFor({ timeout: 8000 });
  pass("backgrounding stops the watcher and scanning; returning to the application catches changes missed while away");

  assert.equal(await page.getByRole("button", { name: "刷新", exact: true }).count(), 0);
  await page.locator('.session-list[aria-busy="false"]').waitFor();
  await page.getByRole("button", { name: "批量管理", exact: true }).click();
  await page.locator(".session-card input[type=checkbox]").first().check();
  before = await count();
  await page.evaluate(() => { window.__AUTO_TEST__.add("qoder", "q4", "Deferred during selection"); window.__AUTO_TEST__.changed(); });
  await page.waitForTimeout(6200);
  assert.equal(await count(), before);
  assert.equal(await page.locator(".session-card input:checked").count(), 1);
  await page.getByRole("button", { name: "完成", exact: true }).click();
  await page.locator(".session-card", { hasText: "Deferred during selection" }).waitFor({ timeout: 2500 });
  pass("automatic updates pause during batch selection and resume afterward without losing the pending change");

  const old = await page.evaluate(() => structuredClone(window.__AUTO_TEST__.watch));
  await page.locator(".agent-item", { hasText: "Codex" }).click();
  await page.locator(".session-card", { hasText: "Existing Codex session" }).waitFor();
  await page.waitForFunction(() => window.__AUTO_TEST__.watch.agent === "codex");
  await page.evaluate(old => window.__AUTO_TEST__.changed(old.agent, old.generation), old);
  assert.equal(await page.locator(".session-card", { hasText: "Qoder" }).count(), 0);
  await page.getByRole("button", { name: "配置", exact: true }).click();
  await page.waitForFunction(() => window.__AUTO_TEST__.watch.agent === null);
  before = await count();
  await page.evaluate(() => { const s = window.__AUTO_TEST__; s.add("codex", "c2", "Created on configuration tab"); s.changed("codex", s.watch.generation - 1); });
  await page.waitForTimeout(1500); assert.equal(await count(), before);
  await page.getByRole("button", { name: "会话", exact: true }).click();
  await page.locator(".session-card", { hasText: "Created on configuration tab" }).waitFor();
  assert.deepEqual(errors, []);
  pass("old Agent events are ignored, the configuration tab does not scan sessions, and returning to sessions loads the new data");
  console.log(`PASS: ${checks} automatic refresh checks (${engine})`);
} catch (error) {
  const artifacts = artifactDir; mkdirSync(artifacts, { recursive: true });
  await page.screenshot({ path: path.join(artifacts, `auto-refresh-failure-${engine}.png`) }).catch(() => {});
  console.error(error); console.error(errors); process.exitCode = 1;
} finally { await browser.close(); }
