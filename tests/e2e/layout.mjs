import { engine, baseURL, artifactDir } from "../helpers/browser.mjs";
import { chromium, webkit } from "playwright";
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { configFixtures, configMockBootstrap } from "../fixtures/config.mjs";


const browser = engine === "webkit" ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: "chrome", headless: true });
const page = await browser.newPage({ viewport: { width: 1600, height: 800 } });
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const artifacts = artifactDir;
mkdirSync(artifacts, { recursive: true });
const storageKey = "agent-session-manager.panel-widths.v1";
let checks = 0;
const pass = (label) => console.log(`PASS ${++checks}: ${label}`);
const sidebarDivider = page.getByRole("separator", { name: "调整侧栏宽度", exact: true });
const listDivider = page.getByRole("separator", { name: "调整会话列表宽度", exact: true });

const sizes = () => page.evaluate(() => Object.fromEntries(["sidebar", "session-list", "detail-pane"].map((name) => [name, document.querySelector(`.${name}`)?.getBoundingClientRect().width ?? 0])));
const expectSizes = async (sidebar, sessions) => page.waitForFunction(({ sidebar, sessions }) =>
  Math.abs(document.querySelector(".sidebar").getBoundingClientRect().width - sidebar) < 1
  && (sessions === undefined || Math.abs(document.querySelector(".session-list").getBoundingClientRect().width - sessions) < 1), { sidebar, sessions });
const saved = () => page.evaluate((key) => localStorage.getItem(key), storageKey);
async function drag(divider, delta) {
  const box = await divider.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + 100);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + delta, box.y + 100, { steps: 6 });
  await page.mouse.up();
}
async function checkBounds() {
  const current = await sizes();
  assert.ok(current.sidebar >= 180 && current["session-list"] >= 260 && current["detail-pane"] >= 360, JSON.stringify(current));
  assert.equal(await page.locator(".main").evaluate((el) => el.scrollWidth <= el.clientWidth), true);
  assert.equal(await page.locator("body").evaluate((el) => el.classList.contains("resizing-panels")), false);
}

try {
  await page.addInitScript(() => {
    const sessions = Array.from({ length: 60 }, (_, i) => ({
      sessionId: `layout-${i}`, projectDir: `project-${i}`, title: `布局测试会话 ${i}`,
      cwd: `/tmp/layout/project-${i}`, model: "auto", firstTs: null, lastTs: null,
      userCount: 1, assistantCount: 1, realUserCount: 1, assistantTextCount: 1,
      sizeBytes: 1024, hasTranscript: true,
    }));
    window.__TAURI_INTERNALS__ = { transformCallback: (cb) => cb, invoke: async (command, args) => {
      if (command === "list_agents") return [
        ["qoder", "Qoder CLI"], ["claude", "Claude Code"], ["codex", "Codex"], ["opencode", "OpenCode"], ["pi", "Pi"],
      ].map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, deletePolicy: "trash" }));
      if (command === "catalog") return { sessions, projects: sessions.map((s) => ({ dirName: s.projectDir, displayPath: s.cwd, sessionCount: 1, lastActivity: null })), warnings: [] };
      if (command === "read_session") return { meta: sessions.find((s) => s.sessionId === args.sessionId), events: [
        { type: "user", uuid: "layout-user", message: { content: "拖拽后对话仍然可读。" } },
        { type: "assistant", uuid: "layout-assistant", message: { content: "右侧正文随可用宽度排版。" } },
      ] };
      throw Error(`Unexpected command: ${command}`);
    } };
  });
  await page.addInitScript(configMockBootstrap, configFixtures);
  await page.goto(baseURL);
  await page.locator(".session-card").first().waitFor();
  assert.equal(await page.getByRole("separator").count(), 2);
  await expectSizes(220, 320);
  await page.locator(".session-card").first().click();
  await page.getByText("拖拽后对话仍然可读。", { exact: true }).waitFor();
  await page.getByRole("searchbox").click();
  await drag(sidebarDivider, 80);
  await expectSizes(300, 320);
  await page.getByRole("searchbox").click();
  await drag(listDivider, 120);
  await expectSizes(300, 440);
  await page.mouse.move(1500, 700);
  for (const divider of [sidebarDivider, listDivider]) {
    assert.equal(await divider.evaluate((el) => getComputedStyle(el, "::before").width), "1px", "鼠标移开后分隔线应恢复细线");
  }
  assert.ok(await page.getByText("拖拽后对话仍然可读。", { exact: true }).isVisible());
  await checkBounds();
  pass("both dividers resize actual panels without replacing the open conversation");

  await page.reload();
  await page.locator(".session-card").first().waitFor();
  await expectSizes(300, 440);
  const preferred = await saved();
  await page.setViewportSize({ width: 960, height: 600 });
  await page.waitForFunction(() => document.querySelector(".sidebar").getBoundingClientRect().width < 300);
  await checkBounds();
  assert.equal(await saved(), preferred);
  await page.setViewportSize({ width: 1600, height: 800 });
  await expectSizes(300, 440);
  pass("reload restores widths; shrinking preserves every minimum and expanding restores preferences");

  await page.getByRole("searchbox").click();
  await page.keyboard.press("Shift+Tab");
  assert.equal(await sidebarDivider.evaluate((el) => document.activeElement === el), true);
  assert.equal(await sidebarDivider.evaluate((el) => getComputedStyle(el, "::before").width), "3px", "键盘导航需要可见焦点");
  await page.keyboard.press("ArrowRight");
  await expectSizes(310, 440);
  await page.keyboard.press("Shift+ArrowLeft");
  await expectSizes(260, 440);
  await sidebarDivider.dblclick();
  await expectSizes(220, 440);
  await listDivider.dblclick();
  await expectSizes(220, 320);
  pass("keyboard resizing and double-click reset work independently for each column");

  await drag(sidebarDivider, -1000);
  await expectSizes(180, 320);
  await drag(listDivider, -1000);
  await expectSizes(180, 260);
  await drag(sidebarDivider, 4000);
  await checkBounds();
  assert.equal(Math.round((await sizes())["detail-pane"]), 360);
  await sidebarDivider.dblclick();
  await drag(listDivider, 4000);
  await checkBounds();
  assert.equal(Math.round((await sizes())["detail-pane"]), 360);
  await listDivider.dblclick();
  await expectSizes(220, 320);
  pass("dragging beyond the viewport respects both narrow-column and conversation minimums");

  const beforeCancel = await saved();
  let box = await sidebarDivider.boundingBox();
  await page.mouse.move(box.x + 4, box.y + 100);
  await page.mouse.down();
  await page.mouse.move(box.x + 74, box.y + 100);
  await expectSizes(290, 320);
  assert.equal(await saved(), beforeCancel, "pointer moves should not write storage");
  await page.keyboard.press("Escape");
  await page.mouse.up();
  await expectSizes(220, 320);
  assert.equal(await saved(), beforeCancel);
  box = await listDivider.boundingBox();
  await page.mouse.move(box.x + 4, box.y + 100);
  await page.mouse.down();
  await page.mouse.move(box.x + 64, box.y + 100);
  await expectSizes(220, 380);
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.mouse.up();
  await expectSizes(220, 320);
  await checkBounds();
  pass("Escape and losing focus cancel an unfinished drag and clear capture/cursor state");

  await drag(sidebarDivider, 40);
  await page.locator(".tabs button", { hasText: "配置" }).click();
  await page.locator(".config-file").first().waitFor();
  assert.equal(await page.getByRole("separator").count(), 1);
  await expectSizes(260);
  await drag(sidebarDivider, 50);
  await expectSizes(310);
  await page.locator(".tabs button", { hasText: "会话" }).click();
  await page.locator(".session-card").first().waitFor();
  await expectSizes(310, 320);
  pass("configuration shares sidebar width while keeping the hidden session-list preference");

  await page.setViewportSize({ width: 960, height: 600 });
  await page.waitForFunction(() => document.querySelector(".sidebar").getBoundingClientRect().width < 310);
  const agentTop = (await page.locator(".sidebar-agents").boundingBox()).y;
  const allTop = (await page.locator(".sidebar-projects-header").boundingBox()).y;
  await page.locator(".sidebar-project-list .project-item").first().hover();
  await page.mouse.wheel(0, 100000);
  await page.waitForFunction(() => document.querySelector(".sidebar-project-list").scrollTop > 1000);
  assert.equal((await page.locator(".sidebar-agents").boundingBox()).y, agentTop);
  assert.equal((await page.locator(".sidebar-projects-header").boundingBox()).y, allTop);
  assert.equal(await page.locator(".sidebar").evaluate((el) => el.scrollTop), 0);
  await page.locator(".sidebar-project-list .project-item").last().click();
  assert.equal(await page.locator(".session-card").count(), 1);
  await checkBounds();
  await page.screenshot({ path: path.join(artifacts, `resizable-layout-${engine}.png`) });
  pass("at the minimum window size Agent navigation stays fixed and projects scroll independently");

  await page.evaluate((key) => localStorage.setItem(key, "{broken"), storageKey);
  await page.reload();
  await page.locator(".session-card").first().waitFor();
  await expectSizes(220, 320);
  await page.evaluate((key) => localStorage.setItem(key, JSON.stringify({ sidebar: -20, sessions: "bad" })), storageKey);
  await page.reload();
  await page.locator(".session-card").first().waitFor();
  await expectSizes(220, 320);
  pass("corrupt or invalid saved widths fall back to usable defaults");

  await page.addInitScript(() => {
    Storage.prototype.getItem = () => { throw Error("mock storage unavailable"); };
    Storage.prototype.setItem = () => { throw Error("mock storage unavailable"); };
  });
  await page.reload();
  await page.locator(".session-card").first().waitFor();
  await expectSizes(220, 320);
  await drag(sidebarDivider, 20);
  await expectSizes(240, 320);
  assert.deepEqual(errors, []);
  pass("unavailable browser storage does not prevent resizing or break rendering");
  console.log(`PASS: ${checks} resizable layout checks (${engine})`);
} catch (error) {
  await page.screenshot({ path: path.join(artifacts, `resizable-layout-failure-${engine}.png`) }).catch(() => {});
  console.error(error);
  process.exitCode = 1;
} finally {
  await browser.close();
}
