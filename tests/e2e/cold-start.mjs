import { addUsageInitScript } from '../helpers/usage-collector.mjs';
import { chooseAgent } from '../helpers/agent-switcher.mjs';
import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import path from 'node:path';

const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = []; page.on('pageerror', e => errors.push(e.message));
  await page.clock.install();
  await addUsageInitScript(page, (_, createCollector) => {
    const names = { qoder: 'Qoder', claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', pi: 'Pi' };
    const state = window.__COLD__ = { calls: [], listeners: {}, registrations: {}, nextListener: 0, active: '', release: null, ready: false, failSummary: false, failEvents: false, failSource: false, synced: [] };
    const tokens = { inputTokens: 4467458800, outputTokens: 21, totalTokens: 4467458821, cacheReadTokens: 0, cacheWriteTokens: 0, reasoningTokens: 0, unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true, cacheReadRecorded: false, cacheWriteRecorded: false, reasoningRecorded: false };
    const model = `claude-${'long-model-name-'.repeat(30)}`;
    const event = { ...tokens, id: 'request', agent: 'claude', time: Date.now(), session: 'session', model, status: 'success', partial: false, outputTps: null, request: { project: `/Users/demo/${'long-directory/'.repeat(30)}project`, client: 'Claude Code', kind: 'cli' } };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    state.collector = createCollector(['qoder', 'claude', 'opencode', 'pi', 'codex'], async agent => {
      state.active = agent;
      await new Promise(resolve => state.release = resolve);
      if (state.failSource) throw Error('Source temporarily unavailable');
      if (!state.synced.includes(agent)) state.synced.push(agent);
      return true;
    });
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: state.collector.bridge(async (cmd, args = {}) => {
      state.calls.push({ cmd, args });
      if (cmd === 'plugin:event|listen') {
        const id = ++state.nextListener; state.registrations[id] = args;
        state.listeners[args.event] = args.handler; return id;
      }
      if (cmd === 'plugin:event|unlisten') {
        if (state.listeners[args.event] === state.registrations[args.eventId]?.handler) delete state.listeners[args.event];
        delete state.registrations[args.eventId]; return;
      }
      if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, deletePolicy: 'trash' }));
      if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (cmd === 'read_usage_summary') {
        if (state.failSummary) throw Error('Database temporarily unavailable');
        return { rows: state.ready ? [{ ...tokens, agent: 'claude', model, date: '', bucketStart: args.query.bounds?.[0] ?? null, partial: false }] : [], syncedAgents: [...state.synced], revision: state.ready ? '1' : '' };
      }
      if (cmd === 'read_request_events') {
        if (state.failEvents) throw Error('Database temporarily unavailable');
        return { events: state.ready ? [event] : [], total: state.ready ? 1 : 0, page: 1, pageSize: args.query.limit ?? 50, revision: state.ready ? '1' : '' };
      }
      return null;
    }) };
    void state.collector.check();
  });
  await page.goto(baseURL);
  await page.getByRole('button', { name: '用量', exact: true }).click();
  await page.waitForFunction(() => window.__COLD__.active === 'qoder');
  await page.locator('.usage-coverage').waitFor();
  assert.equal(await page.locator('.usage-stat[data-metric="tokens"] strong').textContent(), '—');
  assert.equal(await page.getByRole('button', { name: '开启后台采集', exact: true }).count(), 0);
  await page.locator('.usage-sync-trigger').click();
  assert.equal(await page.getByRole('button', { name: '用量同步说明', exact: true }).count(), 0);
  await page.keyboard.press('Escape');
  const emitProgress = (phase, done, total) => page.evaluate(({ phase, done, total }) => {
    const s = window.__COLD__;
    s.collector.progress({ agent: s.active, phase, checkedFiles: done, totalFiles: total });
  }, { phase, done, total });
  await emitProgress('reading', 12, 256);
  await page.getByText(/12 \/ 256 个文件/).waitFor();
  await page.clock.fastForward(65000);
  assert.match(await page.locator('.usage-coverage').textContent(), /正在同步/);
  assert.equal(await page.locator('[role="alert"]').count(), 0, 'long imports must not become timeout errors');
  const assertLayout = async () => {
    for (const width of [960, 1280, 1600]) {
      await page.setViewportSize({ width, height: 900 });
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
      const layout = await page.evaluate(() => {
        const trend = document.querySelector('.usage-trend')?.getBoundingClientRect();
        const breakdowns = document.querySelector('.usage-models')?.getBoundingClientRect();
        return { gap: trend && breakdowns ? breakdowns.top - trend.bottom : 20, widths: [...document.querySelectorAll('.agent-switcher,.usage-models')].map(el => [el.scrollWidth, el.clientWidth]) };
      });
      assert.ok(layout.gap >= 18, `sections touch at ${width}`);
      assert.ok(layout.widths.every(([scroll, client]) => scroll <= client + 1), `breakdown overflow at ${width}`);
    }
  };
  await assertLayout();
  await page.getByRole('tab', { name: '请求记录', exact: true }).click();
  await page.getByText('正在同步历史用量…', { exact: true }).waitFor();
  assert.equal(await page.locator('.request-table').count(), 0, 'an empty table must not create a wide horizontal scrollbar');
  await page.getByRole('tab', { name: '统计分析', exact: true }).click();
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.screenshot({ path: path.join(artifactDir, 'cold-start-loading.png') });
  console.log(`PASS: cold startup remains responsive beyond 65 seconds, shows native progress, and has no empty-table overflow (${engine})`);

  await page.evaluate(() => { window.__COLD__.ready = true; window.__COLD__.release(); });
  await page.waitForFunction(() => window.__COLD__.active === 'claude');
  await page.waitForFunction(() => document.querySelector('.usage-stat[data-metric="tokens"] strong')?.dataset.value === '4467458821');
  assert.equal(await page.locator('.usage-coverage').count(), 1, 'partial results must retain the first-import coverage notice');
  assert.equal(await page.locator('.usage-stat[data-metric="tokens"] > span').textContent(), '已统计 Token');
  assert.match(await page.locator('.usage-coverage').textContent(), /以下为已同步的用量/);
  await chooseAgent(page, 'codex');
  assert.equal(await page.locator('.usage-stat[data-metric="tokens"] strong').textContent(), '—', 'another Agent finishing cannot turn this pending source into measured zero');
  await page.locator('.usage-chart-empty').getByText('正在同步历史用量…', { exact: true }).waitFor();
  await chooseAgent(page, 'all');
  await page.waitForFunction(() => document.querySelector('.usage-stat[data-metric="tokens"] strong')?.dataset.value === '4467458821');
  await assertLayout();
  await page.locator('.usage-trend').screenshot({ path: path.join(artifactDir, 'usage-chart-large-values.png') });
  await page.evaluate(() => { window.__COLD__.failSummary = true; window.__COLD__.failEvents = true; });
  await page.getByRole('button', { name: '今日', exact: true }).click();
  await page.locator('.usage-query-error').waitFor();
  await page.locator('.usage-query-error').getByText(/无法读取用量/).waitFor();
  await page.getByRole('tab', { name: '请求记录', exact: true }).click();
  await page.locator('.request-error').waitFor();
  await page.getByRole('tab', { name: '统计分析', exact: true }).click();
  await page.evaluate(() => { window.__COLD__.failSource = true; window.__COLD__.release(); });
  await page.waitForFunction(() => window.__COLD__.active === 'opencode');
  await page.getByRole('button', { name: '同步状态，同步失败' }).waitFor();
  await assertLayout();
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.screenshot({ path: path.join(artifactDir, 'cold-start-errors.png') });
  await page.evaluate(() => { window.__COLD__.failSummary = false; window.__COLD__.failEvents = false; window.__COLD__.failSource = false; });
  await page.locator('.usage-query-error').getByRole('button', { name: '重试', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('.usage-query-error'));
  for (const next of ['pi', 'codex']) {
    await page.evaluate(() => window.__COLD__.release());
    await page.waitForFunction(next => window.__COLD__.active === next, next);
  }
  await page.evaluate(() => window.__COLD__.release());
  await page.waitForFunction(() => document.querySelector('.usage-coverage')?.textContent.includes('部分 Agent 的首次同步失败'));
  assert.equal(await page.locator('.usage-stat[data-metric="tokens"] > span').textContent(), '已统计 Token', 'a failed source must not turn partial totals into complete totals');
  assert.equal(await page.locator('.usage-stat[data-metric="tokens"] strong').textContent(), '4.47B');
  await page.getByRole('button', { name: '会话', exact: true }).click();
  assert.equal(await page.evaluate(() => window.__COLD__.collector.scans.length), 5);
  assert.deepEqual(errors, []);
  console.log(`PASS: completed-source results, billion-token totals, long paths, and errors/retries stay usable (${engine})`);
} finally { await browser.close(); }
