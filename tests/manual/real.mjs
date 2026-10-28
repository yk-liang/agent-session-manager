import { engine, baseURL } from "../helpers/browser.mjs";
import { chromium, webkit } from "playwright";
import { readFileSync, readdirSync, statSync, mkdtempSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import path from "node:path";

// 失败累计：任何断言失败最终以非零码退出
let failures = 0;
function check(label, ok, detail = "") {
  console.log(`${label}: ${ok ? "OK" : "FAILED"}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures += 1;
}

// 不硬编码具体项目目录：扫描 ~/.qoder/projects 下所有项目，取最大的 jsonl
function findLargestSessionFile() {
  const base = path.join(homedir(), ".qoder", "projects");
  let best = null;
  let bestSize = -1;
  let projectDirs;
  try {
    projectDirs = readdirSync(base);
  } catch {
    return null;
  }
  for (const dir of projectDirs) {
    let files;
    try {
      files = readdirSync(path.join(base, dir));
    } catch {
      continue;
    }
    for (const f of files) {
      if (!f.endsWith(".jsonl")) continue;
      const p = path.join(base, dir, f);
      try {
        const size = statSync(p).size;
        if (size > bestSize) {
          bestSize = size;
          best = { file: p, projectDir: dir };
        }
      } catch {
        continue;
      }
    }
  }
  return best;
}

const found = findLargestSessionFile();
if (!found) {
  console.log("未发现 Qoder 会话文件，跳过真实数据回归");
  process.exit(0);
}
const { file: jsonl, projectDir } = found;
console.log("using session file:", jsonl);

// 截图写入私有临时目录（mkdtemp 默认 0700），避免真实会话内容以 0644 留在 /tmp
const shotDir = mkdtempSync(path.join(tmpdir(), "asm-e2e-real-"));
console.log("screenshots dir:", shotDir);

// 从 shotDir 创建后立即进入 try：无论后续哪一步失败（解析、浏览器启动、
// goto 连不上服务器……），finally 都保证关闭浏览器并删除截图目录
let browser = null;
try {
const events = [];
const meta = {
  sessionId: path.basename(jsonl, ".jsonl"),
  projectDir,
  cwd: null,
  title: null,
  model: null,
  version: null,
  gitBranch: null,
  firstTs: null,
  lastTs: null,
  userCount: 0,
  assistantCount: 0,
  sizeBytes: 0,
  hasTranscript: true,
};
for (const line of readFileSync(jsonl, "utf8").split("\n")) {
  if (!line.trim()) continue;
  let v;
  try {
    v = JSON.parse(line);
  } catch {
    continue;
  }
  if (v.type === "user" || v.type === "assistant" || v.type === "system") events.push(v);
  if (v.type === "ai-title") meta.title = v.aiTitle;
  if (v.type === "custom-title") meta.title = v.customTitle;
  if ((v.type === "user" || v.type === "assistant") && v.timestamp) {
    meta.firstTs ??= v.timestamp;
    meta.lastTs = v.timestamp;
    meta.cwd ??= v.cwd;
    if (v.type === "user") meta.userCount++;
    else meta.assistantCount++;
  }
}
console.log("events:", events.length);

const mock = {
  list_agents: [{ id: "qoder", displayName: "Qoder CLI", installed: true, supportsFork: true, dataDir: "/Users/demo/.qoder", deletePolicy: "trash" }],
  resume_command: "qoder --resume real",
  catalog: {
    projects: [
      { dirName: projectDir, displayPath: "/Users/demo/project", sessionCount: 1, lastActivity: meta.lastTs },
    ],
    sessions: [meta],
    warnings: [],
  },
  read_session: { meta, events },
  get_current_config: {
    agent: "qoder",
    files: [
      { key: "settings.json", label: "settings.json", path: `${homedir()}/.qoder/settings.json`, present: true, content: "{}\n" },
      { key: "mcp.json", label: "mcp.json", path: `${homedir()}/.qoder/mcp.json`, present: true, content: "{\n  \"mcpServers\": {}\n}\n" },
      { key: ".models/default", label: ".models/default（默认模型）", path: `${homedir()}/.qoder/.models/default`, present: false, content: null },
    ],
  },
  list_profiles: [],
};

const initScript = `
  window.__TAURI_INTERNALS__ = {
    invoke: async (cmd) => structuredClone(window.__MOCK__[cmd]),
    transformCallback: (cb) => cb,
  };
`;


  browser =
    engine === "webkit"
      ? await webkit.launch({ headless: true })
      : await chromium.launch({ channel: "chrome", headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  await page.addInitScript(initScript);
  await page.addInitScript({ content: `window.__MOCK__ = ${JSON.stringify(mock)};` });
  const errors = [];
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  page.on("pageerror", (e) => errors.push("PAGEERROR: " + e.message));

  await page.goto(baseURL);
  await page.waitForSelector(".session-card", { timeout: 15000 });
  await page.click(".session-card");
  await page.waitForSelector(".detail-body", { timeout: 15000 });
  await page.waitForTimeout(800);

  const stats = await page.evaluate(() => ({
    collapsibles: document.querySelectorAll(".collapsible").length,
    thinking: document.querySelectorAll(".collapsible.thinking").length,
    tool: document.querySelectorAll(".collapsible.tool").length,
    msgs: document.querySelectorAll(".msg").length,
    systems: document.querySelectorAll(".system-line").length,
    bodyHeight: document.querySelector(".detail-body").scrollHeight,
  }));
  console.log("render stats:", JSON.stringify(stats));
  check(
    "render non-empty",
    events.length > 0 && stats.collapsibles > 0 && stats.msgs > 0,
    `events=${events.length}, collapsibles=${stats.collapsibles}, msgs=${stats.msgs}`,
  );
  await page.screenshot({ path: path.join(shotDir, `real-1-${engine}.png`) });

  // 逐个点击前 5 个折叠卡片，检查展开数量变化
  const n = Math.min(5, stats.collapsibles);
  let opened = 0;
  for (let i = 0; i < n; i++) {
    const head = page.locator(".collapsible-head").nth(i);
    const before = await page.locator(".collapsible-body").count();
    await head.scrollIntoViewIfNeeded();
    await head.click();
    await page.waitForTimeout(150);
    const after = await page.locator(".collapsible-body").count();
    if (after > before) opened++;
  }
  check("collapsible expand", n > 0 && opened === n, `${opened}/${n}`);
  await page.screenshot({ path: path.join(shotDir, `real-2-${engine}.png`) });

  check("no JS errors", errors.length === 0, errors.slice(0, 3).join(" | "));

  // 对话导览验证
  const tocCount = await page.locator(".toc-item").count();
  console.log("toc items:", tocCount);
  if (tocCount > 1) {
    const body = page.locator(".detail-body");
    const before = await body.evaluate((el) => el.scrollTop);
    await page.locator(".toc-item").nth(tocCount - 1).click();
    await page.waitForTimeout(2000);
    const after = await body.evaluate((el) => el.scrollTop);
    check("toc jump", after > before, `scrollTop ${Math.round(before)} -> ${Math.round(after)}`);
    // 跳转后回到顶部附近再点一个折叠卡片，确认命中区域无回归
    await body.evaluate((el) => el.scrollTo(0, 0));
    await page.waitForTimeout(800);
    const bodiesBefore = await page.locator(".collapsible-body").count();
    await page.locator(".collapsible-head").first().click();
    await page.waitForTimeout(200);
    const bodiesAfter = await page.locator(".collapsible-body").count();
    check("collapsible after toc wrap", bodiesAfter !== bodiesBefore, `${bodiesBefore} -> ${bodiesAfter}`);
  }
} finally {
  // 无论成败：关闭浏览器（可能未启动成功）、删除含真实会话内容的截图目录
  if (browser) await browser.close().catch(() => {});
  rmSync(shotDir, { recursive: true, force: true });
}
console.log(failures === 0 ? "DONE" : `DONE with ${failures} failure(s)`);
process.exit(failures === 0 ? 0 : 1);
