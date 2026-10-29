import { chooseAgent } from '../helpers/agent-switcher.mjs';
import { engine, baseURL, artifactDir } from "../helpers/browser.mjs";
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { configFixtures, configMockBootstrap } from "../fixtures/config.mjs";

const SESSION_DETAIL = {
  meta: {
    sessionId: "12345678-1234-4abc-8def-1234567890ab",
    projectDir: "-tmp-demo",
    cwd: "/tmp/demo",
    title: "测试会话",
    model: "mode-xyz",
    modelName: "Qwen3.8-Max-DogFooding",
    version: "1.1.41",
    gitBranch: "main",
    firstTs: "2026-09-03T06:00:00Z",
    lastTs: "2026-09-03T06:35:00Z",
    userCount: 3,
    assistantCount: 4,
    realUserCount: 1,
    assistantTextCount: 3,
    sizeBytes: 12345,
    hasTranscript: true,
  },
  events: [
    {
      type: "user",
      uuid: "u1",
      timestamp: "2026-09-03T06:00:00Z",
      message: { role: "user", content: "帮我写一个快速排序" },
    },
    {
      type: "assistant",
      uuid: "a1",
      timestamp: "2026-09-03T06:01:00Z",
      message: {
        role: "assistant",
        model: "mode-xyz",
        content: [
          null,
          { type: "text", text: { malformed: true } },
          { type: "thinking", thinking: 42 },
          { type: "thinking", thinking: "用户想要快速排序，我用 Python 实现，先写递归版本，注意分区逻辑。" },
          { type: "text", text: "好的，我来实现一个快速排序。" },
          { type: "tool_use", id: "t1", name: "Write", input: { file_path: "/tmp/demo/sort.py", content: "def qsort(xs): ..." } },
        ],
      },
    },
    {
      type: "user",
      uuid: "u2",
      timestamp: "2026-09-03T06:02:00Z",
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "t1", content: "File created successfully at: /tmp/demo/sort.py" }],
      },
    },
    {
      type: "assistant",
      uuid: "a2",
      timestamp: "2026-09-03T06:03:00Z",
      message: {
        role: "assistant",
        model: "mode-xyz",
        content: [
          { type: "tool_use", id: "t2", name: "Bash", input: { command: "python sort_test.py" } },
        ],
      },
    },
    {
      type: "user",
      uuid: "u3",
      timestamp: "2026-09-03T06:04:00Z",
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "t2", is_error: true, content: "command not found: python" }],
      },
    },
    {
      type: "user",
      uuid: "u-orphan",
      timestamp: "2026-09-03T06:04:30Z",
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "missing-tool", content: "孤立工具结果仍应显示" }],
      },
    },
    {
      type: "assistant",
      uuid: "a3",
      timestamp: "2026-09-03T06:05:00Z",
      message: { role: "assistant", model: "mode-xyz", content: [{ type: "text", text: "测试命令失败了，改用 python3 重试。" }] },
    },
    {
      // Codex/OpenCode/Pi 适配器输出的字符串正文回归用例
      type: "assistant",
      uuid: "a4",
      timestamp: "2026-09-03T06:05:30Z",
      message: { role: "assistant", content: "这是字符串形式的助手正文，必须渲染出来。" },
    },
    { type: "system", uuid: "s1", timestamp: "2026-09-03T06:06:00Z", subtype: "informational", content: "Request cancelled." },
  ],
};

const permanentDelete = process.env.PERMANENT_DELETE === "1";
const CLAUDE_CONFIG = {
  agent: "claude",
  files: [
    {
      key: "settings.json",
      label: "settings.json",
      path: "/Users/demo/.claude/settings.json",
      present: true,
      content: "{\n  \"agent\": \"claude\"\n}\n",
    },
  ],
};
const OPENCODE_CONTENT = "{\n" + Array.from({ length: 80 }, (_, i) =>
  `  "setting${i}": "${i === 30 ? "long-value-".repeat(80) : `配置值 ${i}`}"`,
).join(",\n") + "\n}\n";
const OPENCODE_CONFIG = {
  agent: "opencode",
  files: [
    { key: "opencode.jsonc", label: "opencode.jsonc", path: "/Users/demo/.config/opencode/opencode.jsonc", present: false, content: null },
    { key: "opencode.json", label: "opencode.json", path: "/Users/demo/.config/opencode/opencode.json", present: true, content: OPENCODE_CONTENT },
  ],
};
const CODEX_DETAIL = {
  meta: { ...SESSION_DETAIL.meta, sessionId: "codex-1111", title: "Codex 会话", model: "gpt-5.4", modelName: null },
  events: [
    { type: "user", uuid: "cu1", message: { role: "user", content: "codex 用户消息" } },
    { type: "assistant", uuid: "ca1", message: { role: "assistant", content: "codex 助手正文" } },
  ],
};
const mock = {
  list_agents: [
    { id: "qoder", displayName: "Qoder CLI", installed: true, supportsFork: true, dataDir: "/Users/demo/.qoder", deletePolicy: permanentDelete ? "permanent" : "trash" },
    { id: "claude", displayName: "Claude Code", installed: true, supportsFork: false, dataDir: "/Users/demo/.claude", deletePolicy: "trash" },
    { id: "codex", displayName: "Codex", installed: true, supportsFork: false, dataDir: "/Users/demo/.codex", deletePolicy: "trash" },
    { id: "opencode", displayName: "OpenCode", installed: true, supportsFork: false, dataDir: "/Users/demo/.config/opencode", deletePolicy: "permanent" },
    { id: "pi", displayName: "Pi", installed: true, supportsFork: false, dataDir: "/Users/demo/.pi/agent", deletePolicy: "trash" },
  ],
  resume_command: "qoder --resume 12345678-1234-4abc-8def-1234567890ab",
  catalog: {
    projects: [
      { dirName: "-tmp-demo", displayPath: "/tmp/demo", sessionCount: 1, lastActivity: "2026-09-03T06:35:00Z" },
    ],
    sessions: [SESSION_DETAIL.meta],
    warnings: [],
  },
  read_session: SESSION_DETAIL,
  get_current_config: {
    agent: "qoder",
    files: [
      { key: "settings.json", label: "settings.json", path: "/Users/demo/.qoder/settings.json", present: true, content: "{\n  \"hooks\": {}\n}\n" },
      { key: "mcp.json", label: "mcp.json", path: "/Users/demo/.qoder/mcp.json", present: true, content: "{\n  \"mcpServers\": {}\n}\n" },
      { key: ".models/default", label: ".models/default（默认模型）", path: "/Users/demo/.qoder/.models/default", present: false, content: null },
    ],
  },
  list_profiles: [
    { name: "工作", agent: "qoder", createdAt: Date.now(), files: [] },
  ],
  delete_session: null,
  save_profile: { name: "test", agent: "qoder", createdAt: Date.now(), files: [] },
  apply_profile: null,
  update_config_file: null,
};

const initScript = `
  window.__CATALOG_CALLS__ = 0;
  window.__INVOKE_LOG__ = [];
  window.__REVERSE_CONFIG_READS__ = false;
  window.__CONFIG_RESPONSE_SEQ__ = 0;
  window.__HOLD_CONFIG_READS__ = false;
  window.__PENDING_CONFIG_READS__ = [];
  window.__CATALOG_BY_AGENT__ = {};
  window.__DELETE_FAILURES__ = {};
  window.__HOLD_DELETES__ = false;
  window.__PENDING_DELETES__ = [];
  window.__FAIL_NEXT_CATALOG__ = false;
  const mock = ${JSON.stringify(mock)};
  const configByAgent = {
    qoder: structuredClone(mock.get_current_config),
    claude: ${JSON.stringify(CLAUDE_CONFIG)},
    opencode: ${JSON.stringify(OPENCODE_CONFIG)},
  };
  window.__CONFIG_BY_AGENT__ = configByAgent;
  window.__TAURI_INTERNALS__ = {
    invoke: async (cmd, args) => {
      window.__INVOKE_LOG__.push({ cmd, args: structuredClone(args ?? null) });
      const codexDetail = ${JSON.stringify(CODEX_DETAIL)};
      if (cmd === "plugin:clipboard-manager|write_text") { window.__COPIED_TEXT__ = args.text; return null; }
      if (cmd === "search_conversations") return { hits: [], skipped: 0, cancelled: false };
      if (cmd === "cancel_conversation_search") return null;
      if (cmd === "read_session") {
        // 模拟慢请求，制造跨 agent 竞态窗口
        await new Promise((r) => setTimeout(r, 1200));
        return structuredClone(args && args.agent === "codex" ? codexDetail : mock.read_session);
      }
      if (cmd === "catalog") {
        window.__CATALOG_CALLS__ += 1;
        // 保留一个短扫描窗口，验证切换到配置页后 loading 能复位
        await new Promise((r) => setTimeout(r, 200));
        if (window.__FAIL_NEXT_CATALOG__) {
          window.__FAIL_NEXT_CATALOG__ = false;
          throw new Error("mock catalog refresh failure");
        }
        if (window.__CATALOG_BY_AGENT__[args?.agent]) {
          return structuredClone(window.__CATALOG_BY_AGENT__[args.agent]);
        }
        if (args && args.agent === "codex") {
          if (${process.env.CATALOG_FAIL === "1"}) {
            throw new Error("mock catalog failure");
          }
          return structuredClone({
            projects: [{ dirName: "-tmp-codex", displayPath: "/tmp/codex", sessionCount: 1, lastActivity: null }],
            sessions: [codexDetail.meta],
            warnings: [],
          });
        }
        return structuredClone(mock.catalog);
      }
      if (cmd === "delete_session" && window.__CATALOG_BY_AGENT__[args?.agent]) {
        if (window.__HOLD_DELETES__) {
          await new Promise((resolve) => window.__PENDING_DELETES__.push(resolve));
        }
        const error = window.__DELETE_FAILURES__[JSON.stringify([args.agent, args.projectDir, args.sessionId])];
        if (error) throw new Error(error);
        const catalog = window.__CATALOG_BY_AGENT__[args.agent];
        if (!catalog.sessions.some((s) => s.projectDir === args.projectDir && s.sessionId === args.sessionId)) {
          throw new Error("mock session not found");
        }
        catalog.sessions = catalog.sessions.filter((s) => s.projectDir !== args.projectDir || s.sessionId !== args.sessionId);
        catalog.projects = catalog.projects.map((p) => ({
          ...p, sessionCount: catalog.sessions.filter((s) => s.projectDir === p.dirName).length,
        })).filter((p) => p.sessionCount > 0);
        return null;
      }
      if (cmd === "list_profiles" && args && args.agent !== "qoder") {
        return [];
      }
      if (cmd === "get_current_config") {
        const current = structuredClone(configByAgent[args?.agent] ?? mock.get_current_config);
        if (window.__HOLD_CONFIG_READS__) {
          await new Promise((resolve, reject) => window.__PENDING_CONFIG_READS__.push({ resolve, reject }));
        }
        if (window.__REVERSE_CONFIG_READS__) {
          const seq = ++window.__CONFIG_RESPONSE_SEQ__;
          await new Promise((r) => setTimeout(r, seq === 1 ? 300 : 50));
          current.files[0].content = JSON.stringify({ response: seq });
        }
        return current;
      }
      if (cmd === "update_config_file") {
        const file = configByAgent[args.agent]?.files.find((entry) => entry.key === args.key);
        if (!file) throw new Error("unknown config file: " + args.key);
        file.content = args.content;
        file.present = true;
        return null;
      }
      if (!(cmd in mock)) throw new Error("unknown command: " + cmd);
      return structuredClone(mock[cmd]);
    },
    transformCallback: (cb) => cb,
  };
`;


const { webkit } = await import("playwright");
const browser =
  engine === "webkit"
    ? await webkit.launch({ headless: true })
    : await chromium.launch({ channel: "chrome", headless: true });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
await page.addInitScript(initScript + "\n(" + configMockBootstrap.toString() + ")(" + JSON.stringify(configFixtures) + ");");
const pageErrors = [];
page.on("console", (m) => {
  console.log("[console]", m.type(), m.text());
  if (m.type() === "error") pageErrors.push(m.text());
});
page.on("pageerror", (e) => {
  pageErrors.push(e.message);
  console.log("[pageerror]", e.message);
});

await page.goto(baseURL);
await page.waitForSelector(".session-card", { timeout: 10000 });
assert.equal(await page.locator('.session-card .badge').first().textContent(), "Qwen3.8-Max-DogFooding");
assert.ok((await page.locator('.session-card .badge').first().getAttribute('title')).includes('mode-xyz'));
await page.getByRole('searchbox').fill('Qwen3.8-Max-DogFooding');
assert.equal(await page.locator('.session-card').count(), 1);
await page.getByRole('searchbox').fill('');
await page.screenshot({ path: `${artifactDir}/qsm-1-list-${engine}.png` });
console.log("STEP1 list view OK");

// Selecting the current Agent is idempotent; automatic updates own rescanning.
const callsBefore = await page.evaluate(() => window.__CATALOG_CALLS__);
await chooseAgent(page, { label: "Qoder CLI" });
await page.waitForSelector(".session-card", { timeout: 5000 });
const callsAfter = await page.evaluate(() => window.__CATALOG_CALLS__);
const cardsAfter = await page.locator(".session-card").count();
console.log(
  `STEP1f selecting current agent keeps state: calls ${callsBefore} -> ${callsAfter}, cards=${cardsAfter} -> ${
    callsAfter === callsBefore && cardsAfter > 0 ? "OK" : "FAILED"
  }`,
);
if (callsAfter !== callsBefore || cardsAfter === 0) process.exit(1);

// 先正常打开 Qoder 会话
await page.click(".session-card");
await page.waitForSelector(".detail-body .msg", { timeout: 10000 });
assert.equal(await page.locator('.session-id code').textContent(), SESSION_DETAIL.meta.sessionId);
await page.getByRole('button', {name: /^复制会话 ID：/}).click();
assert.equal(await page.evaluate(()=>window.__COPIED_TEXT__), SESSION_DETAIL.meta.sessionId);
assert.equal(await page.locator('.detail-meta .model-label').count(), 0);
await page.getByRole('button', {name:'更多信息',exact:true}).click();
assert.equal(await page.getByRole('group',{name:'会话信息'}).locator('.model-label').textContent(), "Qwen3.8-Max-DogFooding");
assert.ok((await page.getByRole('group',{name:'会话信息'}).textContent()).includes('/tmp/demo'));
await page.keyboard.press('Escape');
assert.equal(await page.getByRole('group',{name:'会话信息'}).count(),0);
// 再次选择会话：旧详情及其操作按钮必须立即不可操作（新请求 1.2s 后才返回）
await page.click(".session-card");
const detailWhileLoading = await page.locator(".detail-body").count();
console.log("STEP1a old detail cleared on re-open:", detailWhileLoading === 0 ? "OK" : "FAILED");
if (detailWhileLoading !== 0) process.exit(1);
await chooseAgent(page, { label: "Codex" });
if (process.env.CATALOG_FAIL === "1") {
  // catalog 失败时不得残留旧 agent 的列表
  await page.waitForTimeout(500);
  const cards = await page.locator(".session-card").count();
  console.log("STEP1d catalog failure leaves no stale list:", cards === 0 ? "OK" : `FAILED (${cards})`);
  await browser.close();
  process.exit(cards === 0 ? 0 : 1);
}
// 切到 Codex 后，旧 agent 的会话卡片必须立即消失
await page.waitForSelector('.session-card:has-text("Codex 会话")', { timeout: 5000 });
assert.equal(await page.locator('.session-card .model-label').textContent(), "gpt-5.4");
const staleCards = await page.locator('.session-card:has-text("测试会话")').count();
console.log("STEP1b stale cards cleared on agent switch:", staleCards === 0 ? "OK" : `FAILED (${staleCards})`);
if (staleCards !== 0) process.exit(1);
await page.waitForTimeout(1800); // 过期响应此时已返回
const raceLeak = await page.locator(".detail-body").count();
console.log("STEP1c race:", raceLeak === 0 ? "OK (过期详情已丢弃)" : "FAILED (旧 agent 详情泄漏)");
if (raceLeak !== 0) process.exit(1);

// 在 Codex 下正常打开会话
await page.click('.session-card:has-text("Codex 会话")');
await page.waitForSelector(".detail-body .msg", { timeout: 10000 });
const codexText = (await page.locator(".detail-body").textContent()) ?? "";
assert.equal(await page.locator('.detail-meta .model-label').count(),0);
await page.getByRole('button',{name:'更多信息',exact:true}).click();
assert.equal(await page.getByRole('group',{name:'会话信息'}).locator('.model-label').textContent(), "gpt-5.4");
await page.locator('.detail-title').click();
assert.equal(await page.getByRole('group',{name:'会话信息'}).count(),0);
console.log("STEP1e codex detail:", codexText.includes("codex 助手正文") ? "OK" : "FAILED");
if (!codexText.includes("codex 助手正文")) process.exit(1);

// 切回 Qoder，继续后续流程
await chooseAgent(page, { label: "Qoder CLI" });
await page.waitForSelector('.session-card:has-text("测试会话")', { timeout: 5000 });

await page.click(".session-card");
await page.waitForSelector(".detail-body .msg", { timeout: 10000 });
await page.screenshot({ path: `${artifactDir}/qsm-2-detail-${engine}.png` });
console.log("STEP2 detail view OK, collapsibles:", await page.locator(".collapsible").count());
const orphanResults = await page.locator('.collapsible.tool .collapsible-head:has-text("工具结果")').count();
console.log(
  `STEP2a malformed/orphan blocks: errors=${pageErrors.length}, orphanResults=${orphanResults} -> ${
    pageErrors.length === 0 && orphanResults === 1 ? "OK" : "FAILED"
  }`,
);
if (pageErrors.length !== 0 || orphanResults !== 1) process.exit(1);

const users = await page.locator(".detail-body .msg.user").count();
const assistants = await page.locator(".detail-body .msg.assistant").count();
console.log(`STEP2b msg counts: users=${users}, assistants=${assistants}`);
if (users < 1 || assistants < 3) {
  console.log("STEP2b FAILED: 助手正文未完整渲染（字符串 content 回归）");
  process.exit(1);
}
console.log("STEP2b OK: 字符串助手正文已渲染");

// 点击第一个折叠卡片（思考过程）
const firstHead = page.locator(".collapsible-head").first();
const firstLabel = await firstHead.textContent();
console.log("STEP3 clicking collapsible:", JSON.stringify(firstLabel));
await firstHead.click();
await page.waitForTimeout(300);
const openBodies = await page.locator(".collapsible-body").count();
console.log("STEP4 open bodies after click:", openBodies);
await page.screenshot({ path: `${artifactDir}/qsm-3-expanded-${engine}.png` });

// 点击工具卡片
const toolHead = page.locator(".collapsible.tool .collapsible-head").first();
if (await toolHead.count() > 0) {
  await toolHead.click();
  await page.waitForTimeout(300);
  console.log("STEP5 tool card clicked, open bodies:", await page.locator(".collapsible-body").count());
  await page.screenshot({ path: `${artifactDir}/qsm-4-tool-${engine}.png` });
}

// 删除确认弹窗验证
const invokeLog = [];
await page.exposeFunction("__logInvoke", (cmd) => invokeLog.push(cmd));
await page.evaluate(() => {
  const orig = window.__TAURI_INTERNALS__.invoke;
  window.__TAURI_INTERNALS__.invoke = async (cmd, ...rest) => {
    await window.__logInvoke(cmd);
    return orig(cmd, ...rest);
  };
});
await page.click(".detail-actions button.danger");
await page.waitForSelector(".modal", { timeout: 3000 });
console.log("STEP6 delete modal shown:", JSON.stringify(await page.locator(".modal-title").textContent()));
const modalBody = (await page.locator(".modal-body").textContent()) ?? "";
const confirmLabel = (await page.locator(".modal-actions button.danger").textContent()) ?? "";
const expectTrash = !permanentDelete;
const bodyOk = expectTrash ? modalBody.includes("回收站") : modalBody.includes("不可恢复");
const labelOk = expectTrash ? confirmLabel.includes("移入回收站") : confirmLabel.includes("永久删除");
console.log(`STEP6b wording: body=${JSON.stringify(modalBody)} label=${JSON.stringify(confirmLabel)} -> ${bodyOk && labelOk ? "OK" : "FAILED"}`);
if (!bodyOk || !labelOk) process.exit(1);
await page.click(".modal-actions button:not(.danger)");
await page.waitForTimeout(200);
console.log("STEP7 cancel closes modal:", (await page.locator(".modal").count()) === 0 ? "OK" : "FAILED");
await page.click(".detail-actions button.danger");
await page.waitForSelector(".modal");
await page.click(".modal-actions button.danger");
await page.waitForTimeout(300);
console.log("STEP8 confirm triggers delete_session:", invokeLog.includes("delete_session") ? "OK" : "FAILED");
const toastText = (await page.locator(".toast").first().textContent()) ?? "";
const toastOk = expectTrash ? toastText.includes("移入回收站") : toastText.includes("永久删除");
console.log("STEP9 toast shown:", toastOk ? `OK (${toastText})` : `FAILED (${toastText})`);
if (!toastOk) process.exit(1);

// 配置编辑、方案预览与恢复、冲突及防丢保护迁入 tests/e2e/config.mjs；
// 此脚本继续覆盖会话与两种标签页间的侧栏、批量操作回归。

const manyProjects = Array.from({ length: 60 }, (_, i) => ({
  dirName: `-tmp-project-${i + 1}`,
  displayPath: `/Users/demo/workspace/projects/project-${i + 1}`,
  sessionCount: 1,
  lastActivity: "2026-10-15T06:00:00Z",
}));
const manySessions = manyProjects.map((project, i) => ({
  ...SESSION_DETAIL.meta,
  sessionId: `sidebar-session-${i + 1}`,
  projectDir: project.dirName,
  cwd: project.displayPath,
  title: `项目 ${i + 1} 的会话`,
  realUserCount: 1,
  assistantTextCount: 1,
}));
await page.evaluate(({ projects, sessions }) => {
  window.__CATALOG_BY_AGENT__.qoder = { projects, sessions, warnings: [] };
  window.__CATALOG_BY_AGENT__.pi = { projects: [], sessions: [], warnings: [] };
}, { projects: manyProjects, sessions: manySessions });
await page.locator(".tabs button", { hasText: "会话" }).click();

for (const viewport of [{ width: 1280, height: 800 }, { width: 960, height: 600 }]) {
  await page.setViewportSize(viewport);
  await chooseAgent(page, { label: "Qoder CLI" });
  await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 60);
  const agentControl = page.getByRole("combobox", { name: "当前 Agent" });
  const allSessions = page.locator(".sidebar .project-item", { hasText: "全部会话" });
  const agentPosition = await agentControl.boundingBox();
  const allPosition = await allSessions.boundingBox();
  await agentControl.click();
  assert.equal(await page.getByRole("option").count(), 5);
  await page.keyboard.press("Escape");
  await page.locator(".sidebar .project-item").nth(1).hover();
  await page.mouse.wheel(0, 100000);
  await page.waitForFunction(() => {
    const items = document.querySelectorAll(".sidebar .project-item");
    const last = items[items.length - 1].getBoundingClientRect();
    const sidebar = document.querySelector(".sidebar").getBoundingClientRect();
    return last.top >= sidebar.top && last.bottom <= sidebar.bottom;
  });
  assert.deepEqual(
    await agentControl.boundingBox(),
    agentPosition,
    "Agent control must remain fixed while scrolling projects",
  );
  assert.equal((await allSessions.boundingBox()).y, allPosition.y, "All sessions must stay fixed");
  assert.equal(await page.locator(".sidebar").evaluate((el) => el.scrollTop), 0);
  const projectScroll = await page.locator(".sidebar-project-list").evaluate((el) => el.scrollTop);
  assert.ok(projectScroll > 1000);

  await page.locator('.sidebar button[title="/Users/demo/workspace/projects/project-60"]').click();
  await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 1);
  assert.ok((await page.locator(".session-card").textContent()).includes("项目 60 的会话"));
  assert.equal(await page.locator(".sidebar-project-list").evaluate((el) => el.scrollTop), projectScroll);
  await page.screenshot({ path: `${artifactDir}/qsm-sidebar-bottom-${viewport.width}-${engine}.png` });

  await page.mouse.click(allPosition.x + allPosition.width / 2, allPosition.y + allPosition.height / 2);
  await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 60);
  assert.equal(await page.locator(".sidebar-project-list").evaluate((el) => el.scrollTop), projectScroll);

  assert.ok(agentPosition.y >= 0 && agentPosition.y + agentPosition.height <= 48);
  await chooseAgent(page, "codex");
  await page.locator('.session-card:has-text("Codex 会话")').waitFor();
  assert.equal(await page.locator(".sidebar-project-list .project-item").count(), 1);
  assert.equal(await page.locator(".sidebar-project-list").evaluate((el) => el.scrollTop), 0);
  assert.equal(await agentControl.getAttribute("value"), "codex");
  console.log(`STEP12 sidebar scroll, fixed actions and agent switch at ${viewport.width}x${viewport.height}: OK`);
}

await chooseAgent(page, { label: "Pi" });
await page.locator(".sidebar .empty-hint", { hasText: "未发现会话" }).waitFor();
await page.getByRole("combobox", { name: "当前 Agent" }).click();
assert.equal(await page.getByRole("option").count(), 5);
await page.keyboard.press("Escape");
assert.equal(await page.locator(".sidebar-project-list .project-item").count(), 0);
assert.ok((await page.locator(".sidebar-projects-header").textContent()).includes("0 个会话"));
await page.locator(".tabs button", { hasText: "配置" }).click();
await page.getByRole("combobox", { name: "当前 Agent" }).click();
assert.equal(await page.getByRole("option").count(), 5);
await page.keyboard.press("Escape");
assert.equal(await page.locator(".sidebar-project-list, .sidebar-projects-header").count(), 0);
await chooseAgent(page, { label: "Claude Code" });
await page.locator(".config-file").waitFor();
assert.equal(await page.getByRole("combobox", { name: "当前 Agent" }).getAttribute("value"), "claude");
assert.deepEqual(pageErrors, []);
console.log("STEP12b empty projects and full-width configuration remain usable: OK");

const batchSessions = [
  { sessionId: "shared-id", projectDir: "project-a", title: "项目 A 的同 ID 会话" },
  { sessionId: "will-fail", projectDir: "project-a", title: "删除失败后重试" },
  { sessionId: "shared-id", projectDir: "project-b", title: "项目 B 的同 ID 会话" },
  { sessionId: "keep", projectDir: "project-b", title: "保留的会话" },
].map((s) => ({ ...SESSION_DETAIL.meta, ...s, cwd: `/tmp/${s.projectDir}`, realUserCount: 1, assistantTextCount: 1 }));
await page.evaluate((sessions) => {
  const projects = ["project-a", "project-b"].map((dirName) => ({
    dirName, displayPath: `/tmp/${dirName}`, sessionCount: 2, lastActivity: null,
  }));
  window.__CATALOG_BY_AGENT__.qoder = { projects, sessions, warnings: [] };
}, batchSessions);
await page.locator(".tabs button", { hasText: "会话" }).click();
await chooseAgent(page, { label: "Qoder CLI" });
await page.getByRole("button", { name: "批量管理", exact: true }).click();
assert.equal(await page.locator(".session-card input[type=checkbox]").count(), 4);
assert.ok(await page.getByRole("button", { name: "删除所选", exact: true }).isDisabled());
await page.locator(".session-card input").first().check();
await page.locator('.search-box input[type="search"]').fill("保留");
assert.equal(await page.locator(".session-card").count(), 1);
assert.equal(await page.locator(".session-card input:checked").count(), 0);
await page.getByRole("button", { name: "全选当前结果", exact: true }).click();
await page.getByRole("button", { name: "删除所选", exact: true }).click();
assert.equal(await page.locator(".batch-targets li").count(), 1);
assert.ok((await page.locator(".batch-targets").textContent()).includes("保留的会话"));
const beforeCancelledDelete = await page.evaluate(() => window.__INVOKE_LOG__.filter((x) => x.cmd === "delete_session").length);
await page.keyboard.press("Escape");
assert.equal(await page.locator(".modal").count(), 0);
assert.equal(await page.evaluate(() => window.__INVOKE_LOG__.filter((x) => x.cmd === "delete_session").length), beforeCancelledDelete);
await page.locator('.search-box input[type="search"]').fill("");
assert.equal(await page.locator(".session-card input:checked").count(), 0);
await page.getByRole("button", { name: "全选当前结果", exact: true }).click();
await page.getByRole("button", { name: "取消全选", exact: true }).click();
assert.equal(await page.locator(".session-card input:checked").count(), 0);
await page.locator(".session-card input").first().check();
await page.locator(".show-empty-toggle input").check();
await page.waitForFunction(() => document.querySelectorAll(".session-card input:checked").length === 0);
await page.locator(".show-empty-toggle input").uncheck();
await page.getByRole("button", { name: "全选当前结果", exact: true }).click();
await page.locator('.sidebar button[title="/tmp/project-a"]').click();
assert.equal(await page.locator(".batch-toolbar").count(), 0);
await page.getByRole("button", { name: "批量管理", exact: true }).click();
await page.getByRole("button", { name: "全选当前结果", exact: true }).click();
assert.equal(await page.locator(".session-card input:checked").count(), 2);
await chooseAgent(page, { label: "Claude Code" });
assert.equal(await page.locator(".batch-toolbar").count(), 0);
await chooseAgent(page, { label: "Qoder CLI" });
await page.getByRole("button", { name: "批量管理", exact: true }).click();
assert.equal(await page.locator(".session-card input:checked").count(), 0);
console.log("STEP13 selection, filtered select-all, confirmation cancellation and context isolation: OK");

await page.getByRole("button", { name: "全选当前结果", exact: true }).click();
await page.locator(".session-card", { hasText: "保留的会话" }).locator("input").uncheck();
assert.equal(await page.locator(".session-card input:checked").count(), 3);
await page.getByRole("button", { name: "删除所选", exact: true }).click();
assert.equal(await page.locator(".batch-targets li").count(), 3);
assert.ok((await page.locator(".modal-body").textContent()).includes("Qoder CLI 的 3 个会话"));
assert.ok((await page.locator(".modal-body").textContent()).includes(permanentDelete ? "永久删除，无法恢复" : "移入系统回收站"));
await page.screenshot({ path: `${artifactDir}/qsm-batch-confirm-${engine}.png` });
const batchStart = await page.evaluate(() => window.__INVOKE_LOG__.filter((x) => x.cmd === "delete_session").length);
const scansBeforeBatch = await page.evaluate(() => window.__CATALOG_CALLS__);
await page.evaluate(() => {
  window.__DELETE_FAILURES__[JSON.stringify(["qoder", "project-a", "will-fail"])] = "mock permission denied";
  window.__HOLD_DELETES__ = true;
  const confirm = document.querySelector(".modal-actions .danger");
  confirm.click();
  confirm.click();
});
await page.waitForFunction(() => window.__PENDING_DELETES__.length === 1);
assert.equal(await page.evaluate(() => window.__INVOKE_LOG__.filter((x) => x.cmd === "delete_session").length), batchStart + 1);
assert.ok((await page.locator(".batch-status").textContent()).includes("0 / 3"));
assert.ok(await page.getByRole("button", { name: "完成", exact: true }).isDisabled());
assert.ok(await page.locator('.search-box input[type="search"]').isDisabled());
assert.ok(await page.locator(".show-empty-toggle input").isDisabled());
assert.equal(await page.getByRole("button", { name: "刷新", exact: true }).count(), 0);
assert.ok(await page.locator(".tabs button", { hasText: "配置" }).isDisabled());
assert.ok(await page.getByRole("combobox", { name: "当前 Agent" }).isDisabled());
assert.ok(await page.locator(".sidebar-projects-header button").isDisabled());
assert.ok(await page.locator(".session-card input").first().isDisabled());
await page.getByRole("combobox", { name: "当前 Agent" }).evaluate(button => {
  button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  button.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
});
assert.equal(await page.getByRole("listbox", { name: "Agent", exact: true }).count(), 0);
assert.equal(await page.getByRole("combobox", { name: "当前 Agent" }).getAttribute("value"), "qoder");
await page.evaluate(() => window.__PENDING_DELETES__.shift()());
await page.waitForFunction((before) => window.__INVOKE_LOG__.filter((x) => x.cmd === "delete_session").length === before + 2 && window.__PENDING_DELETES__.length === 1, batchStart);
assert.ok((await page.locator(".batch-status").textContent()).includes("1 / 3"));
await page.evaluate(() => {
  window.__FAIL_NEXT_CATALOG__ = true;
  window.__HOLD_DELETES__ = false;
  window.__PENDING_DELETES__.shift()();
});
await page.locator(".batch-failures", { hasText: "mock permission denied" }).waitFor();
await page.locator(".catalog-error", { hasText: "mock catalog refresh failure" }).waitFor();
assert.equal(await page.locator(".session-card").count(), 2);
assert.equal(await page.locator(".session-card input:checked").count(), 1);
assert.ok((await page.locator(".sidebar-projects-header").textContent()).includes("2 个会话"));
assert.equal(await page.evaluate(() => window.__CATALOG_CALLS__), scansBeforeBatch + 1);
const attemptedBatch = await page.evaluate((before) => window.__INVOKE_LOG__.filter((x) => x.cmd === "delete_session").slice(before).map((x) => x.args), batchStart);
assert.deepEqual(attemptedBatch, [
  { agent: "qoder", projectDir: "project-a", sessionId: "shared-id" },
  { agent: "qoder", projectDir: "project-a", sessionId: "will-fail" },
  { agent: "qoder", projectDir: "project-b", sessionId: "shared-id" },
]);
await page.screenshot({ path: `${artifactDir}/qsm-batch-partial-${engine}.png` });
await page.evaluate(() => { window.__DELETE_FAILURES__ = {}; });
await page.getByRole("button", { name: "删除所选", exact: true }).click();
assert.equal(await page.locator(".batch-targets li").count(), 1);
await page.locator(".modal-actions .danger").click();
await page.waitForFunction(() => document.querySelectorAll(".session-card").length === 1 && !document.querySelector(".batch-failures"));
assert.ok((await page.locator(".session-card").textContent()).includes("保留的会话"));
assert.equal(await page.locator(".session-card input:checked").count(), 0);
assert.equal(await page.evaluate(() => window.__INVOKE_LOG__.filter((x) => x.cmd === "delete_session").length), batchStart + 4);
assert.ok(await page.getByRole("button", { name: "删除所选", exact: true }).isDisabled());
console.log("STEP13b sequential project-qualified deletion, duplicate-submit protection, partial failure and retry: OK");

await page.getByRole("button", { name: "完成", exact: true }).click();
await page.getByRole("button", { name: "批量管理", exact: true }).click();
await page.getByRole("button", { name: "全选当前结果", exact: true }).click();
await page.locator(".tabs button", { hasText: "配置" }).click();
await page.locator(".tabs button", { hasText: "会话" }).click();
assert.equal(await page.locator(".batch-toolbar").count(), 0);
await page.evaluate((session) => {
  window.__CATALOG_BY_AGENT__.opencode = {
    projects: [{ dirName: session.projectDir, displayPath: session.cwd, sessionCount: 1, lastActivity: null }],
    sessions: [session], warnings: [],
  };
}, batchSessions[3]);
await chooseAgent(page, { label: "OpenCode" });
await page.getByRole("button", { name: "批量管理", exact: true }).click();
await page.getByRole("button", { name: "全选当前结果", exact: true }).click();
await page.getByRole("button", { name: "删除所选", exact: true }).click();
assert.ok((await page.locator(".modal-body").textContent()).includes("会话将永久删除，无法恢复"));
assert.ok((await page.locator(".modal-body").textContent()).includes("关联的子会话"));
assert.ok((await page.locator(".modal-actions .danger").textContent()).includes("永久删除（1）"));
await page.locator(".modal-actions .danger").click();
await page.locator(".session-cards .empty-hint", { hasText: "没有匹配的会话" }).waitFor();
assert.ok(await page.getByRole("button", { name: "全选当前结果", exact: true }).isDisabled());
assert.ok(await page.getByRole("button", { name: "删除所选", exact: true }).isDisabled());
await page.getByRole("button", { name: "完成", exact: true }).click();
assert.ok(await page.getByRole("button", { name: "批量管理", exact: true }).isDisabled());
assert.deepEqual(pageErrors, []);
console.log("STEP13c tab isolation, permanent-delete warning and empty results: OK");

await browser.close();
console.log("DONE");
