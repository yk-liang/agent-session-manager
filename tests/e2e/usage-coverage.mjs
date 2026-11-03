import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import path from 'node:path';
import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chooseAgent } from '../helpers/agent-switcher.mjs';

const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    Object.defineProperty(document, 'hasFocus', { value: () => false });
    const names = { qoder: 'Qoder CLI', claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', pi: 'Pi' };
    const state = window.__COVERAGE__ = { synced: [], calls: [], holdAgent: 'codex', failAgent: null, release: null, holdSummary: false, releaseSummary: null };
    const row = (agent, input) => ({
      agent, model: `${agent}-fixture`, modelName: `${agent}-fixture`, date: '', bucketStart: Date.now() - 1000,
      inputTokens: input, outputTokens: input / 10, totalTokens: input * 1.1, cacheReadTokens: 0, cacheWriteTokens: 0,
      reasoningTokens: 0, unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true,
      cacheReadRecorded: true, cacheWriteRecorded: false, reasoningRecorded: false,
      requestCount: 1, requestCountIncomplete: false, costUsd: null, costIncomplete: false,
      cacheReadIncomplete: false, cacheRateUnavailable: false, partial: false,
    });
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
      state.calls.push({ cmd, args });
      if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, supportsModelNames: false, supportsFastCatalog: false, deletePolicy: 'trash' }));
      if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (cmd === 'collector_status') return { desired: false, running: false, authorization: 'disabled' };
      if (cmd === 'sync_request_events') {
        if (state.holdAgent === args.agent) await new Promise(resolve => state.release = resolve);
        if (state.failAgent === args.agent) throw Error('Source temporarily unavailable');
        if (!state.synced.includes(args.agent)) state.synced.push(args.agent);
        return { updatedAt: Date.now(), checkedFiles: 100, changedFiles: 0, bytesRead: 0 };
      }
      if (cmd === 'read_usage_summary') {
        const result = { rows: [row('claude', 100), row('codex', 1000)].filter(r => state.synced.includes(r.agent)), health: {}, syncedAgents: [...state.synced], revision: String(state.synced.length) };
        if (state.holdSummary) { state.holdSummary = false; await new Promise(resolve => state.releaseSummary = resolve); }
        return result;
      }
      if (cmd === 'read_request_events') return { events: [], models: [], total: 0, page: 1, pageSize: args.query.limit ?? 50, revision: '1' };
      return null;
    } };
  });
  const tab = name => page.getByRole('button', { name, exact: true }).click();
  const waitLabel = label => page.locator('.usage-stat > span').first().filter({ hasText: new RegExp(`^${label}$`) }).waitFor();
  const waitTotal = value => page.waitForFunction(value => document.querySelector('.usage-stat strong')?.textContent === value, value);
  const coverage = page.locator('.usage-coverage');

  await page.goto(baseURL); await tab('用量');
  await page.waitForFunction(() => window.__COVERAGE__.release !== null);
  await waitTotal('110'); await waitLabel('已统计 Token');
  await coverage.getByText('正在统计 · 已完成 4 / 5 个 Agent', { exact: true }).waitFor();
  assert.match(await coverage.textContent(), /Codex/);
  assert.equal(await page.locator('.usage-models tfoot th').textContent(), '已统计合计');
  for (const width of [960, 1280, 1600]) {
    await page.setViewportSize({ width, height: 900 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    assert.ok(await coverage.evaluate(el => el.scrollWidth <= el.clientWidth));
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.screenshot({ path: path.join(artifactDir, 'usage-coverage-partial.png') });

  await chooseAgent(page, 'claude'); await waitLabel('总 Token'); await waitTotal('110');
  assert.equal(await coverage.count(), 0, 'a complete selected Agent is not blocked by another importer');
  await chooseAgent(page, 'codex'); await waitTotal('—'); await waitLabel('已统计 Token');
  await chooseAgent(page, 'qoder'); await waitTotal('0'); await waitLabel('总 Token');
  assert.equal(await coverage.count(), 0, 'confirmed empty sources are measured zero');
  await chooseAgent(page, 'all'); await waitTotal('110'); await waitLabel('已统计 Token');
  const calls = await page.evaluate(() => window.__COVERAGE__.calls.filter(c => c.cmd === 'sync_request_events').length);
  await page.getByRole('button', { name: '今日', exact: true }).click();
  await waitTotal('110'); await waitLabel('已统计 Token');
  assert.equal(await page.evaluate(() => window.__COVERAGE__.calls.filter(c => c.cmd === 'sync_request_events').length), calls);
  console.log(`PASS: partial all-Agent totals stay labeled; completed, pending and confirmed empty sources have distinct states (${engine})`);

  // Leaving and returning must not turn a cached subtotal into complete usage.
  await tab('会话');
  await page.evaluate(() => { window.__COVERAGE__.release = null; });
  await tab('用量');
  await page.waitForFunction(() => window.__COVERAGE__.release !== null);
  await waitTotal('110'); await waitLabel('已统计 Token');
  await page.evaluate(() => { const state = window.__COVERAGE__; state.holdSummary = true; state.release(); });
  await page.waitForFunction(() => window.__COVERAGE__.releaseSummary !== null && window.__COVERAGE__.synced.length === 5);
  assert.equal(await page.locator('.usage-stat strong').first().textContent(), '110');
  assert.equal(await page.locator('.usage-stat > span').first().textContent(), '已统计 Token');
  await coverage.getByText('正在汇总用量…', { exact: true }).waitFor();
  await page.evaluate(() => window.__COVERAGE__.releaseSummary());
  await waitTotal('1,210'); await waitLabel('总 Token');
  assert.equal(await coverage.count(), 0);
  assert.equal(await page.locator('.usage-models tfoot th').textContent(), '合计');
  console.log(`PASS: cached partial data stays partial across tabs; completion waits for the matching summary snapshot (${engine})`);

  // A completed saved baseline is immediately usable even if the next sync is slow.
  await tab('会话');
  await page.evaluate(() => { window.__COVERAGE__.release = null; });
  await tab('用量');
  await page.waitForFunction(() => window.__COVERAGE__.release !== null);
  await waitTotal('1,210'); await waitLabel('总 Token');
  await page.waitForTimeout(400);
  assert.equal(await page.locator('.usage-sync-status,.usage-spin').count(), 0);
  console.log(`PASS: complete saved history remains visible without first-import animation on later visits (${engine})`);

  // Start a fresh app with no baseline and fail the last Agent's first import.
  await page.goto(baseURL); await tab('用量');
  await page.waitForFunction(() => window.__COVERAGE__.release !== null);
  await page.evaluate(() => { const state = window.__COVERAGE__; state.failAgent = 'codex'; state.release(); });
  await page.locator('.usage-source-error').waitFor();
  await coverage.getByText('统计尚未完整', { exact: true }).waitFor();
  await waitTotal('110'); await waitLabel('已统计 Token');
  await chooseAgent(page, 'codex'); await waitTotal('—');
  assert.equal(await page.locator('.usage-spin').count(), 0, 'failure must not leave an endless loading animation');
  await chooseAgent(page, 'all');
  await page.evaluate(() => { const state = window.__COVERAGE__; state.failAgent = null; state.holdAgent = null; });
  await page.getByRole('button', { name: '重试同步', exact: true }).click();
  await waitTotal('1,210'); await waitLabel('总 Token');
  assert.equal(await coverage.count(), 0);
  assert.equal(await page.getByRole('alert').count(), 0);
  assert.deepEqual(errors, []);
  console.log(`PASS: first-import failures retain the subtotal, never show unmeasured zero, and recover on retry (${engine})`);
} finally { await browser.close(); }
