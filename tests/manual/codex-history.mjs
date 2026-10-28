import { engine, baseURL } from "../helpers/browser.mjs";
import { chromium, webkit } from "playwright";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

// Rust 只读合并真实日志；导出只存在于 0700 临时目录中的 0600 文件，结束时清理。
const privateDir = mkdtempSync(path.join(tmpdir(), "asm-codex-history-"));
const exported = path.join(privateDir, "detail.json");

let browser;
try {
  const run = await promisify(execFile)("cargo", ["test", "--offline", "--locked", "real_paginated_history_is_unique_and_complete_if_present", "--", "--nocapture"], {
    cwd: path.resolve("src-tauri"), env: { ...process.env, ASM_TEST_CODEX_EXPORT: exported }, timeout: 180000,
  });
  console.log((run.stdout + run.stderr).split("\n").filter((line) => /test result:|Codex merged history:/.test(line)).join("\n"));
  const detail = JSON.parse(readFileSync(exported, "utf8"));
  browser = engine === "webkit" ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: "chrome", headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("crash", () => errors.push("Browser page crashed"));
  await page.exposeFunction("__READ_HISTORY__", () => detail);
  const lastIndex = detail.events.findLastIndex((e) => e.type === "assistant" && typeof e.message?.content === "string" && e.message.content.trim());
  const lastText = detail.events[lastIndex].message.content;
  const query = lastText.trim().slice(0, 18);
  await page.addInitScript(({ meta, lastIndex, query, eventId }) => {
    window.__TAURI_INTERNALS__ = { transformCallback: (cb) => cb, invoke: async (command) => {
      if (command === "list_agents") return [{ id: "codex", displayName: "Codex CLI", installed: true, supportsFork: false, deletePolicy: "trash" }];
      if (command === "catalog") return { sessions: [meta], projects: [{ dirName: meta.projectDir, displayPath: meta.cwd, sessionCount: 1, lastActivity: meta.lastTs }], warnings: [] };
      if (command === "read_session") return window.__READ_HISTORY__();
      if (command === "search_conversations") return { hits: [{ sessionId: meta.sessionId, projectDir: meta.projectDir, role: "assistant", eventIndex: lastIndex, eventId, snippet: query }], skipped: 0, cancelled: false };
      if (command === "cancel_conversation_search") return null;
      throw Error(`Unexpected command: ${command}`);
    } };
  }, { meta: detail.meta, lastIndex, query, eventId: detail.events[lastIndex].uuid ?? null });
  await page.goto(baseURL);
  await page.locator(".session-card").waitFor();
  assert.equal(await page.locator(".session-card").count(), 1);
  await page.locator(".session-card").click();
  await page.getByRole("button", { name: `复制会话 ID：${detail.meta.sessionId}`, exact: true }).waitFor({ timeout: 60000 });
  assert.ok(await page.locator(".event-group").count() < 60, "长会话只挂载可见行");
  assert.ok(await page.locator(".msg.user").count() > 0);
  assert.ok(await page.locator(".msg.assistant").count() > 0);
  console.log(`PASS: real merged history virtualizes ${detail.events.length} events (${engine})`);

  await page.getByRole("searchbox").fill(query);
  await page.locator(".session-search-snippet").waitFor();
  await page.locator(".session-card").click();
  const mark = page.locator(`#ev-${lastIndex}.search-target mark`).first();
  await mark.waitFor({ timeout: 60000 });
  const hit = await mark.boundingBox();
  const body = await page.locator(".detail-body").boundingBox();
  assert.ok(hit.y >= body.y && hit.y + hit.height <= body.y + body.height);
  assert.deepEqual(errors, []);
  console.log(`PASS: search reaches and highlights the last merged page (${engine})`);
} finally {
  await browser?.close();
  rmSync(privateDir, { recursive: true, force: true });
}
