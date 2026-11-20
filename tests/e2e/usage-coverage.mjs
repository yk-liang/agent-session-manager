import { addUsageInitScript } from '../helpers/usage-collector.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import path from 'node:path';
import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chooseAgent } from '../helpers/agent-switcher.mjs';

const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await addUsageInitScript(page, (_, createCollector) => {
    Object.defineProperty(document, 'hasFocus', { value: () => false });
    const names = { qoder: 'Qoder', claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', pi: 'Pi' };
    const state = window.__COVERAGE__ = { synced: [], calls: [], holdAgent: 'codex', failAgent: null, release: null };
    const row = (agent, input) => ({
      agent, model: `${agent}-fixture`, date: '', bucketStart: Date.now() - 1000,
      inputTokens: input, outputTokens: input / 10, totalTokens: input * 1.1, cacheReadTokens: 0, cacheWriteTokens: 0,
      reasoningTokens: 0, unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true,
      cacheReadRecorded: true, cacheWriteRecorded: false, reasoningRecorded: false,
      requestCount: 1, requestCountIncomplete: false, costUsd: null, costIncomplete: false,
      cacheReadIncomplete: false, partial: false,
      cacheInputTokens: input, cacheInputReadTokens: 0, cacheInputWriteTokens: 0, cacheExcludedCount: 0,
    });
    state.collector = createCollector(['qoder', 'claude', 'opencode', 'pi', 'codex'], async agent => {
      if (state.holdAgent === agent) await new Promise(resolve => state.release = resolve);
      if (state.failAgent === agent) throw Error('Source temporarily unavailable');
      if (state.synced.includes(agent)) return false;
      state.synced.push(agent);
      return true;
    });
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: state.collector.bridge(async (cmd, args = {}) => {
      state.calls.push({ cmd, args });
      if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, deletePolicy: 'trash' }));
      if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (cmd === 'read_usage_summary') {
        return { rows: [row('claude', 100), row('codex', 1000)].filter(r => state.synced.includes(r.agent)),
          syncedAgents: [...state.synced], revision: String(state.synced.length) };
      }
      if (cmd === 'read_request_events') return { events: [], total: 0, page: 1, pageSize: args.query.limit ?? 50, revision: '1' };
      return null;
    }) };
    void state.collector.check();
  });
  const tab = name => page.getByRole('button', { name, exact: true }).click();
  const waitLabel = label => page.locator('.usage-stat[data-metric="tokens"] > span').filter({ hasText: new RegExp(`^${label}$`) }).waitFor();
  const waitTotal = value => page.waitForFunction(value => document.querySelector('.usage-stat[data-metric="tokens"] strong')?.textContent === value, value);
  const coverage = page.locator('.usage-coverage');

  await page.goto(baseURL); await tab('用量');
  await page.waitForFunction(() => window.__COVERAGE__.release !== null);
  await waitTotal('110'); await waitLabel('已统计 Token');
  await coverage.getByText(/正在同步 Codex/).waitFor();
  assert.match(await coverage.textContent(), /Codex/);
  assert.equal(await page.locator('.usage-models tfoot th').textContent(), '已同步小计');
  for (const width of [960, 1280, 1600]) {
    await page.setViewportSize({ width, height: 900 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    assert.ok(await coverage.evaluate(el => el.scrollWidth <= el.clientWidth));
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.screenshot({ path: path.join(artifactDir, 'usage-coverage-partial.png') });

  await chooseAgent(page, 'claude'); await waitLabel('Token 用量'); await waitTotal('110');
  assert.equal(await coverage.count(), 0, 'a complete selected Agent is not blocked by another importer');
  await chooseAgent(page, 'codex'); await waitTotal('—'); await waitLabel('已统计 Token');
  await chooseAgent(page, 'qoder'); await waitTotal('0'); await waitLabel('Token 用量');
  assert.equal(await coverage.count(), 0, 'confirmed empty sources are measured zero');
  await chooseAgent(page, 'all'); await waitTotal('110'); await waitLabel('已统计 Token');
  const calls = await page.evaluate(() => window.__COVERAGE__.collector.scans.length);
  await page.getByRole('button', { name: '今日', exact: true }).click();
  await waitTotal('110'); await waitLabel('已统计 Token');
  assert.equal(await page.evaluate(() => window.__COVERAGE__.collector.scans.length), calls);
  console.log(`PASS: partial all-Agent totals stay labeled; completed, pending and confirmed empty sources have distinct states (${engine})`);

  // Finish the same collector job away from the Usage tab.
  await tab('会话');
  await page.evaluate(() => window.__COVERAGE__.release());
  await page.waitForFunction(() => window.__COVERAGE__.synced.length === 5);
  await tab('用量');
  await waitTotal('1.21K'); await waitLabel('Token 用量');
  assert.equal(await coverage.count(), 0);
  assert.equal(await page.locator('.usage-models tfoot th').textContent(), '合计');
  assert.equal(await page.evaluate(() => window.__COVERAGE__.collector.scans.length), 5);
  console.log(`PASS: collection finishes away from Usage; reopening reads the new snapshot without starting another scan (${engine})`);

  // A completed saved baseline is immediately usable even if the next sync is slow.
  await tab('会话');
  await page.evaluate(() => { const state = window.__COVERAGE__; state.release = null; void state.collector.check(); });
  await tab('用量');
  await page.waitForFunction(() => window.__COVERAGE__.release !== null);
  await waitTotal('1.21K'); await waitLabel('Token 用量');
  await page.waitForTimeout(400);
  assert.equal(await page.locator('.usage-coverage,.usage-spin').count(), 0);
  console.log(`PASS: complete saved history remains visible without first-import animation on later visits (${engine})`);

  // Start a fresh app with no baseline and fail the last Agent's first import.
  await page.goto(baseURL); await tab('用量');
  await page.waitForFunction(() => window.__COVERAGE__.release !== null);
  await page.evaluate(() => { const state = window.__COVERAGE__; state.failAgent = 'codex'; state.release(); });
  const syncFailure = page.getByRole('button', { name: '同步状态，同步失败' });
  await syncFailure.waitFor();
  await coverage.getByText(/部分 Agent 的首次同步失败/).waitFor();
  await waitTotal('110'); await waitLabel('已统计 Token');
  await chooseAgent(page, 'codex'); await waitTotal('—');
  assert.equal(await page.locator('.usage-spin').count(), 0, 'failure must not leave an endless loading animation');
  await chooseAgent(page, 'all');
  await page.evaluate(() => { const state = window.__COVERAGE__; state.failAgent = null; state.holdAgent = null; });
  await syncFailure.click();
  await page.getByRole('button', { name: '重试同步', exact: true }).click();
  await waitTotal('1.21K'); await waitLabel('Token 用量');
  assert.equal(await coverage.count(), 0);
  assert.equal(await page.getByRole('alert').count(), 0);
  assert.deepEqual(errors, []);
  console.log(`PASS: first-import failures retain the subtotal, never show unmeasured zero, and recover on retry (${engine})`);
} finally { await browser.close(); }
