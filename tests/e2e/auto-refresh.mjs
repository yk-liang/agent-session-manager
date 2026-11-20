import { chooseAgent } from '../helpers/agent-switcher.mjs';
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
      firstTs: null, lastTs: null, userCount: 1, assistantCount: 1, realUserCount: 1, assistantTextCount: 1,
      sizeBytes: 100, hasTranscript: true });
    const state = { rows: { qoder: [row("q1", "Existing Qoder session")], codex: [row("c1", "Existing Codex session")] },
      calls: [], catalogs: [], watch: { agent: null, generation: 0 }, callbacks: {}, listeners: {}, next: 0,
      // Codex 式两阶段目录：preview 中的会话只有文件头，refine_catalog 返回完整统计。
      preview: new Set(), events: {}, refining: false, releaseRefine: null };
    const headerOnly = session => ({ ...session, title: null, model: null, userCount: 0, assistantCount: 0,
      realUserCount: 0, assistantTextCount: 0, statsPending: true });
    const listing = (agent, preview) => {
      const sessions = structuredClone(state.rows[agent]).map(s => preview && state.preview.has(s.sessionId) ? headerOnly(s) : s);
      return { sessions, projects: [{ dirName: "real-source", displayPath: "/tmp/real-source", sessionCount: sessions.length, lastActivity: null }], warnings: [] };
    };
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
          { id: "qoder", displayName: "Qoder", installed: true, supportsFork: false, deletePolicy: "trash" },
          { id: "codex", displayName: "Codex", installed: true, supportsFork: false, deletePolicy: "trash" }];
        if (command === "watch_sessions") {
          if (args.generation >= state.watch.generation) state.watch = structuredClone(args);
          return null;
        }
        if (command === "catalog") {
          state.catalogs.push({ agent: args.agent, at: performance.now() });
          return listing(args.agent, true);
        }
        if (command === "refine_catalog") {
          state.refining = true;
          await new Promise(resolve => { state.releaseRefine = resolve; });
          state.refining = false; state.preview.clear();
          return listing(args.agent, false);
        }
        if (command === "read_tool_payload") return { type: "tool_result", tool_use_id: "call-mid", content: `payload ${args.token}` };
        if (command === "search_conversations") return { hits: [], skipped: 0, cancelled: false };
        if (command === "get_config_workspace") return { agent: args.agent, revision: "fixture", files: [], profiles: [],
          matchingProfiles: [], overview: { model: null, provider: null, endpoint: null, mcpServers: [] }, backups: [], warnings: [] };
        if (command === "read_session") return { meta: structuredClone(state.rows[args.agent].find(s => s.sessionId === args.sessionId)),
          events: structuredClone(state.events[args.sessionId]
            ?? [{ type: "assistant", uuid: "stable", message: { content: "Keep the open conversation stable" } }]) };
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
  await page.waitForFunction(() => document.querySelector(".session-card")?.textContent.includes("9 条消息"), undefined, { timeout: 8000 });
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
  await chooseAgent(page, { label: "Codex" });
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

  // 正在写入的 Codex 会话：目录先发布只有文件头的预览，再在后台补齐统计；打开的正文随文件变化重读。
  await page.evaluate(() => {
    const s = window.__AUTO_TEST__;
    const confirmed = (id, title) => ({ sessionId: id, title, projectDir: "real-source", cwd: "/tmp/real-source", model: "logged-model",
      firstTs: null, lastTs: "2026-10-01T00:00:00Z", userCount: 3, assistantCount: 4, realUserCount: 3, assistantTextCount: 4,
      sizeBytes: 1000, hasTranscript: true });
    s.rows.codex = [confirmed("live", "Live session"), ...Array.from({ length: 40 }, (_, i) => confirmed(`f${i}`, `Filler ${i}`))];
    s.events.live = Array.from({ length: 200 }, (_, i) => i === 100
      ? { type: "assistant", uuid: "tool", message: { content: [{ type: "tool_use", id: "call-mid", name: "shell", input: { cmd: "ls" } }] } }
      : i === 101 ? { type: "user", uuid: "result", message: { content: [{ type: "tool_result", tool_use_id: "call-mid", deferred: "tok-mid" }] } }
      : { type: i % 20 ? "assistant" : "user", uuid: `e${i}`, message: { content: `Message ${i}\n` + "body line\n".repeat(4) } });
    s.changed();
  });
  await page.locator(".session-card", { hasText: "Live session" }).waitFor({ timeout: 15000 });
  const reads = () => page.evaluate(() => window.__AUTO_TEST__.calls.filter(call => call.command === "read_session").length);
  const payloads = () => page.evaluate(() => window.__AUTO_TEST__.calls.filter(call => call.command === "read_tool_payload").length);
  const stable = async read => {
    let last = await read();
    for (let i = 0; i < 40; i++) {
      await page.waitForTimeout(80);
      const next = await read();
      if (JSON.stringify(next) === JSON.stringify(last)) return next;
      last = next;
    }
    throw new Error(`layout did not settle: ${JSON.stringify(last)}`);
  };
  const detailPosition = () => stable(() => page.evaluate(() => {
    const body = document.querySelector(".detail-body"); const head = document.querySelector("#ev-100 .collapsible-head");
    return { top: body.scrollTop, head: head ? Math.round(head.getBoundingClientRect().top - body.getBoundingClientRect().top) : null,
      expanded: head?.getAttribute("aria-expanded") ?? null };
  }));
  const listAnchor = () => stable(() => page.evaluate(() => {
    const list = document.querySelector(".session-cards"); const top = list.getBoundingClientRect().top;
    const card = [...list.querySelectorAll(".session-card-row")].map(row => ({ row, rect: row.getBoundingClientRect() }))
      .filter(({ rect }) => rect.bottom > top).sort((a, b) => a.rect.top - b.rect.top)[0];
    return { title: card.row.querySelector(".session-card-title").textContent, offset: Math.round(card.rect.top - top) };
  }));

  await page.locator(".session-card", { hasText: "Live session" }).locator(".session-card-open").click();
  await page.locator(".detail-title", { hasText: "Live session" }).waitFor();
  await page.waitForFunction(() => {
    const body = document.querySelector(".detail-body"); body.scrollTop += 400; return !!document.querySelector("#ev-100");
  }, undefined, { polling: 50, timeout: 8000 });
  await page.evaluate(() => document.querySelector("#ev-100").scrollIntoView({ block: "center" }));
  await page.locator("#ev-100 .collapsible-head").click();
  await page.getByText("payload tok-mid", { exact: true }).waitFor();
  const reading = await detailPosition();
  assert.ok(reading.top > 1000 && reading.expanded === "true", JSON.stringify(reading));
  const payloadCalls = await payloads();
  await page.evaluate(() => {
    document.querySelector(".detail").dataset.mounted = "original";
    const jitter = window.__JITTER__ = [];
    new MutationObserver(() => {
      if (!document.querySelector('.detail[data-mounted="original"]')) jitter.push("detail replaced or cleared");
      for (const title of document.querySelectorAll(".session-card-title")) {
        const card = title.closest(".session-card");
        if (title.textContent === "无标题") jitter.push("card lost its title");
        else if (!card.textContent.includes("条消息") || !card.querySelector(".model-label")) jitter.push(`${title.textContent} lost count or model`);
      }
    }).observe(document.body, { subtree: true, childList: true, characterData: true });
  });

  before = await reads();
  await page.locator(".session-card", { hasText: "Live session" }).locator(".session-card-open").click();
  await page.waitForFunction(count => window.__AUTO_TEST__.calls.filter(call => call.command === "read_session").length > count, before);
  assert.deepEqual(await detailPosition(), reading);
  assert.deepEqual(await page.evaluate(() => window.__JITTER__), []);
  pass("reopening the same conversation keeps its body, reading position and expanded tool in place");

  await page.locator(".session-cards").evaluate(list => { list.scrollTop = 1000; });
  const anchor = await listAnchor();
  before = await reads();
  await page.evaluate(() => {
    const s = window.__AUTO_TEST__; const rows = s.rows.codex;
    const live = rows.find(row => row.sessionId === "live"); const touched = rows.find(row => row.sessionId === "f13");
    Object.assign(live, { sizeBytes: 3000, lastTs: "2026-10-02T00:00:00Z", realUserCount: 4, assistantTextCount: 5 });
    Object.assign(touched, { sizeBytes: 2000, lastTs: "2026-10-02T00:00:00Z", realUserCount: 6 });
    rows.unshift({ ...touched, sessionId: "inserted", title: "Inserted above" });
    s.events.live.push({ type: "user", uuid: "appended-user", message: { content: "Appended question" } },
      { type: "assistant", uuid: "appended", message: { content: "Appended after refresh" } });
    s.preview = new Set(["live", "f13", "inserted"]);
    s.changed();
  });
  await page.waitForFunction(() => window.__AUTO_TEST__.refining, undefined, { timeout: 15000 });
  assert.deepEqual(await page.evaluate(() => window.__JITTER__), [], "header-only preview must not blank known card details");
  assert.deepEqual(await listAnchor(), anchor, "rows inserted above must not move the card being read");
  const f13 = page.locator(".session-card", { hasText: "Filler 13" });
  assert.match(await f13.textContent(), /7 条消息/);
  assert.equal(await f13.locator(".model-label").count(), 1);
  assert.equal(await page.locator(".detail-title").textContent(), "Live session");
  assert.equal(await reads(), before, "the open body waits for settled statistics before re-reading a large history");
  await page.evaluate(() => window.__AUTO_TEST__.releaseRefine());
  await f13.filter({ hasText: "10 条消息" }).waitFor();
  await page.waitForFunction(count => window.__AUTO_TEST__.calls.filter(call => call.command === "read_session").length === count + 1, before);
  await page.waitForFunction(() => document.querySelector(".toc-title")?.textContent.includes("10 条提问"), undefined, { timeout: 8000 });
  assert.deepEqual(await detailPosition(), reading, "content updates keep the reading position and expanded tool");
  assert.deepEqual(await listAnchor(), anchor);
  assert.equal(await payloads(), payloadCalls, "already loaded tool payloads are reused after the body is re-read");
  assert.deepEqual(await page.evaluate(() => window.__JITTER__), []);
  await page.locator(".detail-body").evaluate(body => { body.scrollTop = body.scrollHeight; });
  await page.getByText("Appended after refresh", { exact: true }).waitFor();
  pass("preview then refined statistics keep card title, model and count; the open body updates once in place without jumping");

  await page.locator(".session-cards").evaluate(list => { list.scrollTop = 0; });
  before = await reads();
  await page.evaluate(() => {
    const s = window.__AUTO_TEST__; const rows = s.rows.codex;
    rows.unshift({ ...rows.find(row => row.sessionId === "f0"), sessionId: "newest", title: "Newest on top" });
    s.changed();
  });
  await page.waitForFunction(() => document.querySelector(".session-card-title")?.textContent === "Newest on top", undefined, { timeout: 30000 });
  assert.equal(await reads(), before, "an unchanged open session is not re-read");
  assert.deepEqual(await page.evaluate(() => window.__JITTER__), []);
  assert.deepEqual(errors, []);
  pass("at the top the list still shows the newest order, and catalog changes elsewhere do not re-read the open body");
  console.log(`PASS: ${checks} automatic refresh checks (${engine})`);
} catch (error) {
  const artifacts = artifactDir; mkdirSync(artifacts, { recursive: true });
  await page.screenshot({ path: path.join(artifacts, `auto-refresh-failure-${engine}.png`) }).catch(() => {});
  console.error(error); console.error(errors); process.exitCode = 1;
} finally { await browser.close(); }
