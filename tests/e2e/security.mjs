import { engine } from "../helpers/browser.mjs";
import { availablePort } from "../helpers/server.mjs";
import { preview } from "vite";
import { chromium, webkit } from "playwright";
import { readFile } from "node:fs/promises";
import assert from "node:assert/strict";
import { configFixtures, configMockBootstrap } from "../fixtures/config.mjs";

const configuration = JSON.parse(await readFile(new URL("../../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
const policy = configuration.app.security.csp;
const csp = typeof policy === "string" ? policy : policy && Object.entries(policy).map(([directive, sources]) => `${directive} ${Array.isArray(sources) ? sources.join(" ") : sources}`).join("; ");

const server = await preview({ preview: { host: "127.0.0.1", port: await availablePort(), strictPort: true, headers: csp ? { "Content-Security-Policy": csp } : {} } });
let browser;
try {
  browser = engine === "webkit" ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: "chrome", headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("https://csp-test.invalid/**", (route) => route.abort());
  await page.addInitScript(() => {
    window.__CSP_TEST__ = { inlineExecuted: false, violations: [] };
    document.addEventListener("securitypolicyviolation", (event) => {
      window.__CSP_TEST__.violations.push({ directive: event.effectiveDirective, uri: event.blockedURI });
    });
    const meta = { sessionId: "security-fixture", projectDir: "fixture-project", cwd: "/tmp/fixture", title: "Security fixture", model: "fixture-model", firstTs: null, lastTs: null, realUserCount: 1, assistantTextCount: 1, userCount: 1, assistantCount: 1, sizeBytes: 100, hasTranscript: true };
    window.__TAURI_INTERNALS__ = { transformCallback: (callback) => callback, invoke: async (command) => {
      if (command === "list_agents") return [{ id: "qoder", displayName: "Qoder", installed: true, supportsFork: true, deletePolicy: "trash" }];
      if (command === "catalog") return { projects: [{ dirName: meta.projectDir, displayPath: meta.cwd, sessionCount: 1, lastActivity: null }], sessions: [meta], warnings: [] };
      if (command === "read_session") return { meta, events: [{ type: "user", message: { content: "Read-only question" } }, { type: "assistant", message: { content: [{ type: "text", text: "# Local markdown\n\n**Bold content** and `code`.\n\n![External image](https://csp-test.invalid/markdown.png)" }] } }] };
      if (command === "search_conversations") return { hits: [], skipped: 0, cancelled: false };
      if (command === "usage_sync_status") return { sequence: 1, revision: 1, updatedAt: Date.now(), syncing: [], progress: null, failedAgents: [], unavailable: false };
      if (command === "read_usage_summary") return { rows: [], projects: [], providers: [], syncedAgents: ["qoder"], revision: "1", pricingRevision: 1 };
      if (command === "read_request_events") return { events: [], total: 0, page: 1, pageSize: 50, revision: "1" };
      if (command === "get_model_prices") return { models: [], catalog: { revision: 1, sources: [{ id: "models.dev", name: "Models.dev", url: "https://models.dev/api.json", fetchedAt: null, modelCount: 0, pricedCount: 0, stale: true, error: null }, { id: "litellm", name: "LiteLLM", url: "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json", fetchedAt: null, modelCount: 0, pricedCount: 0, stale: true, error: null }], customCount: 0, exchangeRateCnyPerUsd: 7 } };
      return null;
    } };
  });
  await page.addInitScript(configMockBootstrap, configFixtures);
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/`);
  await page.locator(".session-card").click();
  await page.locator(".detail-title").waitFor();
  const markdown = page.getByRole("checkbox", { name: /Markdown/ });
  await markdown.check();
  await page.getByRole("heading", { name: "Local markdown", exact: true }).waitFor();
  assert.equal(await page.locator('img[src^="https://"]').count(), 0);
  await page.getByRole("button", { name: "配置", exact: true }).click();
  const file = page.locator('.config-file[data-file-key="settings.json"]');
  await file.getByRole("button", { name: "编辑", exact: true }).click();
  await file.locator("textarea").fill('{"model":"csp-fixture"}');
  await file.getByRole("button", { name: "保存", exact: true }).click();
  await page.getByText("csp-fixture", { exact: true }).waitFor();
  await page.getByRole("button", { name: "用量", exact: true }).click();
  await page.getByRole("tab", { name: "统计分析", exact: true }).waitFor();
  await page.getByRole("tab", { name: "请求记录", exact: true }).click();
  await page.getByRole("tab", { name: "模型价格", exact: true }).click();
  await page.locator(".prices-panel").waitFor();
  assert.deepEqual(errors, []);
  assert.deepEqual(await page.evaluate(() => window.__CSP_TEST__.violations), []);
  await page.evaluate(() => {
    const script = document.createElement("script");
    script.textContent = "window.__CSP_TEST__.inlineExecuted = true";
    document.body.append(script);
    const image = new Image();
    image.src = "https://csp-test.invalid/injected.png";
    document.body.append(image);
  });
  assert.equal(await page.evaluate(() => window.__CSP_TEST__.inlineExecuted), false, "Production CSP must block injected inline scripts");
  await page.waitForFunction(() => window.__CSP_TEST__.violations.some((v) => v.directive.startsWith("script-src")) && window.__CSP_TEST__.violations.some((v) => v.directive === "img-src"));
  assert.ok(csp.includes("ipc:"), "Native IPC must remain permitted");
  assert.ok(csp.includes("http://ipc.localhost"), "HTTP-based native IPC must remain permitted");
  assert.deepEqual(errors, []);
  console.log(`PASS: production CSP, lazy Markdown, configuration editing and blocked injection (${engine})`);
} finally {
  await browser?.close();
  await new Promise((resolve, reject) => server.httpServer.close((error) => error ? reject(error) : resolve()));
}
