import { chooseAgent } from '../helpers/agent-switcher.mjs';
import { engine, baseURL } from "../helpers/browser.mjs";
import { chromium, webkit } from "playwright";
import assert from "node:assert/strict";

const browser = engine === "webkit" ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: "chrome", headless: true });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
const errors = []; page.on("pageerror", error => errors.push(error.message));
let checks = 0; const pass = label => console.log(`PASS ${++checks}: ${label}`);
try {
  await page.addInitScript(() => {
    Object.defineProperty(document, "hasFocus", { value: () => false });
    window.addEventListener("focus", event => { if (!window.__CACHE_TEST__?.allowFocus) event.stopImmediatePropagation(); });
    const row = (id, title, model = null) => ({ sessionId: id, projectDir: "source-dir", cwd: "/tmp/source", title,
      model, modelName: null, firstTs: null, lastTs: null, realUserCount: 1, assistantTextCount: 1,
      userCount: 1, assistantCount: 1, sizeBytes: 100, hasTranscript: true });
    const state = { rows: { qoder: [row("q1", "Qoder question", "mode-id")], codex: [row("c1", "Codex question")] },
      hold: {}, pending: [], calls: [], naming: [], named: false, deleted: [] };
    const catalog = (agent, empty) => {
      const sessions = [...state.rows[agent], ...(empty ? [{ ...row("empty", "Empty session"), realUserCount: 0 }] : [])];
      return structuredClone({ sessions, projects: sessions.length ? [{ dirName: "source-dir", displayPath: "/tmp/source", sessionCount: sessions.length, lastActivity: null }] : [], warnings: [] });
    };
    state.release = (agent, fail = false) => {
      const targets = state.pending.filter(p => p.agent === agent);
      state.pending = state.pending.filter(p => p.agent !== agent);
      for (const p of targets) fail ? p.reject(Error("scan unavailable")) : p.resolve(catalog(agent, p.empty));
    };
    state.add = () => state.rows.qoder.push(row("q2", "New Qoder question", "mode-id"));
    window.__CACHE_TEST__ = state;
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (command, args = {}) => {
      state.calls.push({ command, args: structuredClone(args) });
      if (command === "list_agents") return [
        { id: "qoder", displayName: "Qoder CLI", installed: true, supportsFork: true, supportsModelNames: true, deletePolicy: "trash" },
        { id: "codex", displayName: "Codex", installed: true, supportsFork: false, deletePolicy: "trash" }];
      if (command === "catalog") {
        if (state.hold[args.agent]) return new Promise((resolve, reject) => state.pending.push({ agent: args.agent, empty: args.includeEmpty, resolve, reject }));
        return catalog(args.agent, args.includeEmpty);
      }
      if (command === "resolve_model_names") {
        if (state.named) return { "mode-id": "Qwen readable name" };
        const result = await new Promise(resolve => state.naming.push(resolve));
        state.named = true; return result;
      }
      if (command === "read_session") return { meta: state.rows[args.agent].find(s => s.sessionId === args.sessionId),
        events: [{ type: "user", message: { content: "Read-only fixture question" } }] };
      if (command === "delete_session") {
        state.deleted.push(args.sessionId); state.rows[args.agent] = state.rows[args.agent].filter(s => s.sessionId !== args.sessionId); return null;
      }
      if (command === "search_conversations") return { hits: [], skipped: 0, cancelled: false };
      return null;
    } };
  });
  const qoder = () => chooseAgent(page, "qoder");
  const codex = () => chooseAgent(page, "codex");
  const cards = page.locator(".session-card");
  const reload = () => page.evaluate(() => {
    window.__CACHE_TEST__.allowFocus = true;
    window.dispatchEvent(new Event("blur")); window.dispatchEvent(new Event("focus"));
    window.__CACHE_TEST__.allowFocus = false;
  });
  const waitLoaded = () => page.locator('.session-list[aria-busy="false"]').waitFor();
  const project = page.locator('.sidebar-project-list .project-item[title="/tmp/source"]');
  const assertProjectDetail = async (phase) => {
    assert.equal(await project.and(page.locator(".active")).count(), 1, `${phase}: project selection must survive`);
    assert.equal(await page.locator(".detail-title").textContent(), "Qoder question", `${phase}: open detail must survive`);
    assert.equal(await cards.filter({ hasText: /^Qoder question/ }).and(page.locator(".active")).count(), 1);
    assert.ok(await page.getByRole("button", { name: "删除", exact: true }).isEnabled());
  };
  await page.goto(baseURL);
  await cards.first().waitFor();
  await page.waitForFunction(() => window.__CACHE_TEST__.naming.length === 1);
  assert.equal(await cards.count(), 1);
  pass("the first catalog renders while the model registry request remains unresolved");

  await codex(); await cards.filter({ hasText: "Codex question" }).waitFor();
  await page.evaluate(() => { for (const resolve of window.__CACHE_TEST__.naming) resolve({ "mode-id": "Qwen readable name" }); });
  await page.waitForFunction(() => window.__CACHE_TEST__.named);
  assert.equal(await cards.filter({ hasText: "Qwen readable name" }).count(), 0);
  await page.evaluate(() => { window.__CACHE_TEST__.hold.qoder = true; window.__CACHE_TEST__.hold.codex = true; });
  await qoder();
  await cards.filter({ hasText: "Qwen readable name" }).waitFor({ timeout: 750 });
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.some(p => p.agent === "qoder"));
  pass("a late name response updates the Qoder cache without leaking into another Agent; switching back uses it before refresh finishes");

  await codex(); await cards.filter({ hasText: "Codex question" }).waitFor({ timeout: 750 });
  await qoder(); await cards.filter({ hasText: "Qoder question" }).waitFor({ timeout: 750 });
  assert.equal(await page.evaluate(() => window.__CACHE_TEST__.pending.filter(p => p.agent === "qoder").length), 1);
  await page.evaluate(() => { window.__CACHE_TEST__.add(); window.__CACHE_TEST__.release("qoder"); });
  await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 2);
  await page.evaluate(() => window.__CACHE_TEST__.release("codex"));
  await waitLoaded();
  assert.equal(await cards.filter({ hasText: "Codex question" }).count(), 0);
  pass("rapid switching coalesces scans, background refresh adds new sessions, and stale Agent responses cannot replace the active list");

  await project.click();
  await cards.filter({ hasText: /^Qoder question/ }).click();
  await page.locator(".detail-title").waitFor();
  await reload();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  await assertProjectDetail("cached refresh pending");
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder", true));
  await waitLoaded();
  await page.locator(".catalog-error").waitFor();
  assert.equal(await cards.count(), 2);
  await assertProjectDetail("cached refresh failed");
  await reload();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  await waitLoaded();
  assert.equal(await cards.filter({ hasText: "Qwen readable name" }).count(), 2);
  await assertProjectDetail("cached refresh retried");
  pass("refresh failure retains project, detail and cached list; retry retains readable model names");

  await page.getByLabel("显示空会话").check();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  await assertProjectDetail("uncached empty-session filter pending");
  assert.equal(await cards.count(), 2);
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder", true));
  await waitLoaded();
  await assertProjectDetail("uncached empty-session filter failed");
  assert.equal(await cards.count(), 2);
  await reload();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  await assertProjectDetail("uncached empty-session filter retry pending");
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 3);
  await waitLoaded();
  await assertProjectDetail("uncached empty-session filter loaded");
  pass("first empty-session filter preserves project and detail during loading, failure and successful retry");

  await page.getByLabel("显示空会话").uncheck();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 2, undefined, { timeout: 750 });
  await assertProjectDetail("cached filter switch pending");
  assert.equal(await cards.count(), 2);
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  await waitLoaded();
  await assertProjectDetail("cached filter switch loaded");
  pass("empty-session filters use separate snapshots and switch back without losing project or detail");

  await cards.filter({ hasText: /^Qoder question/ }).click();
  await page.locator(".detail-title").waitFor();
  await page.getByRole("button", { name: "删除", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "移入回收站", exact: true }).click();
  await page.waitForFunction(() => window.__CACHE_TEST__.deleted.length === 1);
  assert.equal(await cards.count(), 1);
  await codex(); await qoder();
  await cards.filter({ hasText: "New Qoder question" }).waitFor();
  assert.equal(await cards.count(), 1);
  await page.evaluate(() => { window.__CACHE_TEST__.release("qoder"); window.__CACHE_TEST__.release("codex"); });
  await waitLoaded();
  assert.equal(await cards.count(), 1);
  pass("deletion evicts both cached filters immediately and switching cannot revive the deleted card");

  await project.click();
  await cards.filter({ hasText: "New Qoder question" }).click();
  await page.locator(".detail-title").waitFor();
  await page.getByLabel("显示空会话").check();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.some(p => p.agent === "qoder"));
  await codex();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.some(p => p.agent === "codex"));
  assert.equal(await cards.count(), 0, "uncached Agent must not inherit the previous Agent's cards");
  assert.equal(await page.locator(".detail-actions").count(), 0);
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  assert.equal(await cards.count(), 0);
  assert.equal(await page.locator(".detail-actions").count(), 0);
  await page.evaluate(() => window.__CACHE_TEST__.release("codex", true));
  await waitLoaded();
  assert.equal(await cards.count(), 0);
  assert.equal(await page.locator(".detail-actions").count(), 0);
  await reload();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.some(p => p.agent === "codex"));
  assert.equal(await page.locator(".detail-actions").count(), 0);
  await page.evaluate(() => window.__CACHE_TEST__.release("codex"));
  await cards.filter({ hasText: "Codex question" }).waitFor();
  assert.equal(await cards.filter({ hasText: "Qoder" }).count(), 0);
  await cards.filter({ hasText: "Codex question" }).click();
  await page.locator(".detail-title", { hasText: "Codex question" }).waitFor();
  assert.equal(await page.getByRole("button", { name: "分叉会话", exact: true }).count(), 0);
  await qoder();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.some(p => p.agent === "qoder"));
  assert.equal(await cards.filter({ hasText: "Codex question" }).count(), 0);
  assert.equal(await page.locator(".detail-actions").count(), 0);
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  await waitLoaded();
  await page.getByLabel("显示空会话").uncheck();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.some(p => p.agent === "qoder"));
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  await waitLoaded();
  pass("uncached Agent loading, failure, retry and late responses never expose the previous Agent's cards or actions");

  await project.click();
  await cards.filter({ hasText: "New Qoder question" }).click();
  await page.locator(".detail-title").waitFor();
  await reload();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  assert.equal(await project.and(page.locator(".active")).count(), 1);
  assert.equal(await page.locator(".detail-title").textContent(), "New Qoder question");
  await page.evaluate(() => { window.__CACHE_TEST__.rows.qoder = []; window.__CACHE_TEST__.release("qoder"); });
  await waitLoaded();
  await page.locator(".sidebar-projects-header .project-item.active").waitFor();
  assert.equal(await cards.count(), 0);
  assert.equal(await project.count(), 0);
  assert.equal(await page.locator(".detail-actions").count(), 0);
  pass("a confirmed empty catalog clears a vanished project and its detail instead of retaining stale actions");

  await page.reload();
  await cards.first().waitFor();
  await project.click();
  await cards.filter({ hasText: /^Qoder question/ }).click();
  await page.locator(".detail-title").waitFor();
  await page.evaluate(() => { window.__CACHE_TEST__.hold.qoder = true; });
  await page.getByLabel("显示空会话").check();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  await assertProjectDetail("first successful filter pending");
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 2);
  await waitLoaded();
  await assertProjectDetail("first successful filter loaded");
  assert.deepEqual(errors, []);
  pass("first filter success also preserves project and detail in a fresh workspace; no browser runtime errors");
  console.log(`PASS: ${checks} catalog cache checks (${engine})`);
} catch (error) {
  console.error(error); process.exitCode = 1;
} finally { await browser.close(); }
