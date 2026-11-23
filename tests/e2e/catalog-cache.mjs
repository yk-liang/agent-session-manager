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
      model, firstTs: null, lastTs: null, realUserCount: 1, assistantTextCount: 1,
      userCount: 1, assistantCount: 1, sizeBytes: 100, hasTranscript: true });
    const state = { rows: { qoder: [row("q1", "Qoder question", "mode-id")], codex: [row("c1", "Codex question")] },
      hold: {}, pending: [], calls: [], deleted: [], cancelled: [] };
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
        { id: "qoder", displayName: "Qoder", installed: true, supportsFork: true, deletePolicy: "trash" },
        { id: "codex", displayName: "Codex", installed: true, supportsFork: false, deletePolicy: "trash" }];
      if (command === "cancel_catalog") {
        const stale = state.pending.filter(request => request.generation < args.generation);
        state.pending = state.pending.filter(request => request.generation >= args.generation);
        for (const request of stale) {
          state.cancelled.push(request.agent);
          request.reject(Error("catalog cancelled"));
        }
        return null;
      }
      if (command === "catalog") {
        if (state.hold[args.agent]) return new Promise((resolve, reject) => state.pending.push({
          agent: args.agent, empty: args.includeEmpty, generation: args.generation, resolve, reject,
        }));
        return catalog(args.agent, args.includeEmpty);
      }
      if (command === "read_session") return { meta: state.rows[args.agent].find(s => s.sessionId === args.sessionId),
        events: [{ type: "user", message: { content: "Read-only fixture question" } }] };
      if (command === "resume_command") return `qoder --resume ${args.sessionId}`;
      if (command === "plugin:clipboard-manager|write_text") { state.copied = args.text; return null; }
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
  assert.equal(await cards.count(), 1);
  await codex(); await cards.filter({ hasText: "Codex question" }).waitFor();
  await page.evaluate(() => { window.__CACHE_TEST__.hold.qoder = true; window.__CACHE_TEST__.hold.codex = true; });
  await qoder();
  await cards.filter({ hasText: "Qoder question" }).waitFor({ timeout: 750 });
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.some(p => p.agent === "qoder"));
  pass("switching back displays the matching cached catalog before refresh finishes");

  await codex(); await cards.filter({ hasText: "Codex question" }).waitFor({ timeout: 750 });
  await qoder(); await cards.filter({ hasText: "Qoder question" }).waitFor({ timeout: 750 });
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.filter(p => p.agent === "qoder").length === 1);
  assert.ok(await page.evaluate(() => window.__CACHE_TEST__.cancelled.includes("qoder") && window.__CACHE_TEST__.cancelled.includes("codex")));
  await page.evaluate(() => { window.__CACHE_TEST__.add(); window.__CACHE_TEST__.release("qoder"); });
  await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 2);
  await page.evaluate(() => window.__CACHE_TEST__.release("codex"));
  await waitLoaded();
  assert.equal(await cards.filter({ hasText: "Codex question" }).count(), 0);
  pass("rapid switching cancels previous scans, and the latest refresh updates the matching list");

  await project.click();
  await cards.filter({ hasText: /^Qoder question/ }).click();
  await page.locator(".detail-title").waitFor();
  await reload();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  await assertProjectDetail("cached refresh pending");
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder", true));
  await waitLoaded();
  await page.getByRole("alert", { name: "无法读取会话列表", exact: true }).waitFor();
  assert.equal(await cards.count(), 2);
  await assertProjectDetail("cached refresh failed");
  await reload();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  await waitLoaded();
  assert.equal(await cards.count(), 2);
  await assertProjectDetail("cached refresh retried");
  pass("refresh failure and retry retain the project, detail and cached list");

  await page.getByLabel("空会话").check();
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

  await page.getByLabel("空会话").uncheck();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.length === 1);
  await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 2, undefined, { timeout: 750 });
  await assertProjectDetail("cached filter switch pending");
  assert.equal(await cards.count(), 2);
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  await waitLoaded();
  await assertProjectDetail("cached filter switch loaded");
  pass("empty-session filters use separate snapshots and switch back without losing project or detail");

  await cards.filter({ hasText: "New Qoder question" }).click();
  await page.locator(".detail-title").waitFor();
  const readsBeforeShortcuts = await page.evaluate(() => window.__CACHE_TEST__.calls.filter(call => call.command === "read_session").length);
  const targetCard = cards.filter({ hasText: /^Qoder question/ });
  const shortcuts = targetCard.getByRole("group", { name: "会话快捷操作" });
  const cardTime = targetCard.locator(".session-card-time");
  assert.ok(await cardTime.isVisible());
  await targetCard.hover();
  assert.equal(await cardTime.isVisible(), false);
  const copy = shortcuts.getByRole("button", { name: "复制恢复命令（Qoder question）" });
  await copy.hover();
  assert.ok(await copy.locator(".session-action-tooltip", { hasText: "复制恢复命令" }).isVisible());
  await copy.click();
  await page.waitForFunction(() => window.__CACHE_TEST__.copied === "qoder --resume q1");
  await shortcuts.getByRole("button", { name: "在终端恢复（Qoder question）" }).click();
  await page.waitForFunction(() => window.__CACHE_TEST__.calls.some(({ command, args }) =>
    command === "resume_in_terminal" && args.agent === "qoder" && args.sessionId === "q1" && args.cwd === "/tmp/source" && !args.fork));
  await shortcuts.getByRole("button", { name: "删除（Qoder question）" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "移入废纸篓", exact: true }).click();
  await page.waitForFunction(() => window.__CACHE_TEST__.deleted.length === 1);
  assert.equal(await cards.count(), 1);
  assert.equal(await page.locator(".detail-title").textContent(), "New Qoder question");
  assert.equal(await page.evaluate(() => window.__CACHE_TEST__.calls.filter(call => call.command === "read_session").length), readsBeforeShortcuts);
  await codex(); await qoder();
  await cards.filter({ hasText: "New Qoder question" }).waitFor();
  assert.equal(await cards.count(), 1);
  await page.evaluate(() => { window.__CACHE_TEST__.release("qoder"); window.__CACHE_TEST__.release("codex"); });
  await waitLoaded();
  assert.equal(await cards.count(), 1);
  pass("card shortcuts act without opening the target, and deletion evicts both cached filters while preserving the open detail");

  await project.click();
  await cards.filter({ hasText: "New Qoder question" }).click();
  await page.locator(".detail-title").waitFor();
  await page.getByLabel("空会话").check();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.some(p => p.agent === "qoder"));
  await codex();
  await page.waitForFunction(() => window.__CACHE_TEST__.pending.some(p => p.agent === "codex"));
  const assertCodexFallback = async () => {
    assert.equal(await cards.count(), 1);
    assert.equal(await cards.filter({ hasText: "Codex question" }).count(), 1);
    assert.equal(await page.locator(".detail-actions").count(), 0);
  };
  await assertCodexFallback();
  await page.evaluate(() => window.__CACHE_TEST__.release("qoder"));
  await assertCodexFallback();
  await page.evaluate(() => window.__CACHE_TEST__.release("codex", true));
  await waitLoaded();
  await assertCodexFallback();
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
  await page.getByLabel("空会话").uncheck();
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
  await page.getByLabel("空会话").check();
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
