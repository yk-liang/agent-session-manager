import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import path from 'node:path';

const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = []; page.on('pageerror', e => errors.push(e.message));
  await page.clock.install();
  await page.addInitScript(() => {
    const names = { qoder: 'Qoder CLI', claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', pi: 'Pi' };
    const state = window.__COLD__ = { calls: [], listeners: {}, registrations: {}, nextListener: 0, active: '', generation: 0, release: null, ready: false, failSummary: false, failEvents: false, failSource: false };
    const tokens = { inputTokens: 4467458800, outputTokens: 21, totalTokens: 4467458821, cacheReadTokens: 0, cacheWriteTokens: 0, reasoningTokens: 0, unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true, cacheReadRecorded: false, cacheWriteRecorded: false, reasoningRecorded: false };
    const model = `claude-${'long-model-name-'.repeat(30)}`;
    const event = { ...tokens, id: 'request', agent: 'claude', time: Date.now(), session: 'session', model, modelName: model, status: 'success', partial: false, outputTps: null, request: { project: `/Users/demo/${'long-directory/'.repeat(30)}project`, client: 'Claude Code', kind: 'cli' } };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
      state.calls.push({ cmd, args });
      if (cmd === 'plugin:event|listen') {
        const id = ++state.nextListener; state.registrations[id] = args;
        state.listeners[args.event] = args.handler; return id;
      }
      if (cmd === 'plugin:event|unlisten') {
        if (state.listeners[args.event] === state.registrations[args.eventId]?.handler) delete state.listeners[args.event];
        delete state.registrations[args.eventId]; return;
      }
      if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, supportsFastCatalog: false, supportsModelNames: false, deletePolicy: 'trash' }));
      if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (cmd === 'collector_status') return { desired: false, running: false, authorization: 'signingRequired' };
      if (cmd === 'sync_request_events') {
        state.active = args.agent; state.generation = args.generation;
        await new Promise(resolve => state.release = resolve);
        if (state.failSource) throw Error('Source temporarily unavailable');
        return { updatedAt: Date.now(), checkedFiles: 256, changedFiles: 256, bytesRead: 270000000 };
      }
      if (cmd === 'read_usage_summary') {
        if (state.failSummary) throw Error('Database temporarily unavailable');
        return { rows: state.ready ? [{ ...tokens, agent: 'claude', model, modelName: model, date: '', bucketStart: args.query.bounds?.[0] ?? null, partial: false }] : [], health: {}, revision: state.ready ? '1' : '' };
      }
      if (cmd === 'read_request_events') {
        if (state.failEvents) throw Error('Database temporarily unavailable');
        return { events: state.ready ? [event] : [], models: state.ready ? [model] : [], total: state.ready ? 1 : 0, nextCursor: null, reset: false, revision: state.ready ? '1' : '' };
      }
      return null;
    } };
  });
  await page.goto(baseURL);
  await page.getByRole('button', { name: '用量', exact: true }).click();
  await page.waitForFunction(() => window.__COLD__.active === 'qoder');
  await page.locator('.usage-sync-status').waitFor();
  assert.equal(await page.locator('.usage-stat strong').first().textContent(), '—');
  await page.getByText('正在读取历史请求…', { exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: '开启后台采集', exact: true }).count(), 0);
  await page.getByRole('button', { name: '同步与保留说明', exact: true }).click();
  assert.ok((await page.getByRole('tooltip').textContent()).includes('此版本不支持退出后的后台采集'));
  await page.keyboard.press('Escape');
  const emitProgress = (phase, done, total) => page.evaluate(({ phase, done, total }) => {
    const s = window.__COLD__;
    s.listeners['usage-sync-progress']({ payload: { generation: s.generation, progress: { agent: s.active, phase, checkedFiles: done, totalFiles: total } } });
  }, { phase, done, total });
  await emitProgress('reading', 12, 256);
  await page.getByText('已检查 12 / 256 个文件', { exact: true }).waitFor();
  await page.clock.fastForward(65000);
  assert.match(await page.locator('.usage-sync-status').textContent(), /已用时 6[5-9] 秒/);
  assert.equal(await page.locator('[role="alert"]').count(), 0, 'long imports must not become timeout errors');
  assert.equal(await page.locator('.request-table').count(), 0, 'an empty table must not create a wide horizontal scrollbar');
  const assertLayout = async () => {
    for (const width of [960, 1280, 1600]) {
      await page.setViewportSize({ width, height: 900 });
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
      const layout = await page.evaluate(() => {
        const requests = document.querySelector('.request-events').getBoundingClientRect();
        const breakdowns = document.querySelector('.usage-breakdowns').getBoundingClientRect();
        return { gap: breakdowns.top - requests.bottom, widths: [...document.querySelectorAll('.agent-switcher,.usage-models')].map(el => [el.scrollWidth, el.clientWidth]) };
      });
      assert.ok(layout.gap >= 18, `sections touch at ${width}`);
      assert.ok(layout.widths.every(([scroll, client]) => scroll <= client + 1), `breakdown overflow at ${width}`);
    }
  };
  await assertLayout();
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.screenshot({ path: path.join(artifactDir, 'cold-start-loading.png') });
  console.log(`PASS: cold startup remains responsive beyond 65 seconds, shows native progress, and has no empty-table overflow (${engine})`);

  await page.evaluate(() => { window.__COLD__.ready = true; window.__COLD__.release(); });
  await page.waitForFunction(() => window.__COLD__.active === 'claude');
  await page.waitForFunction(() => document.querySelector('.usage-stat strong')?.textContent === '4,467,458,821');
  assert.equal(await page.locator('.usage-sync-status').count(), 0, 'once saved results are available, remaining sources update quietly');
  await page.getByRole('combobox', { name: '当前 Agent' }).selectOption('codex');
  assert.equal(await page.locator('.usage-stat strong').first().textContent(), '—', 'another Agent finishing cannot turn this pending source into measured zero');
  await page.getByText('正在同步历史用量…', { exact: true }).waitFor();
  await page.getByRole('combobox', { name: '当前 Agent' }).selectOption('all');
  await page.waitForFunction(() => document.querySelector('.usage-stat strong')?.textContent === '4,467,458,821');
  await assertLayout();
  await page.locator('.usage-trend').screenshot({ path: path.join(artifactDir, 'usage-chart-large-values.png') });
  await page.evaluate(() => { window.__COLD__.failSummary = true; window.__COLD__.failEvents = true; });
  await page.getByRole('button', { name: '今日', exact: true }).click();
  await page.locator('.usage-query-error').waitFor(); await page.locator('.request-error').waitFor();
  await page.getByText('暂时无法读取用量，请重试查询', { exact: true }).waitFor();
  await page.evaluate(() => { window.__COLD__.failSource = true; window.__COLD__.release(); });
  await page.waitForFunction(() => window.__COLD__.active === 'opencode');
  await page.locator('.usage-source-error').waitFor();
  await assertLayout();
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.screenshot({ path: path.join(artifactDir, 'cold-start-errors.png') });
  await page.evaluate(() => { window.__COLD__.failSummary = false; window.__COLD__.failEvents = false; window.__COLD__.failSource = false; });
  await page.getByRole('button', { name: '重试查询', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('.usage-query-error') && !document.querySelector('.request-error'));
  for (const next of ['pi', 'codex']) {
    await page.evaluate(() => window.__COLD__.release());
    await page.waitForFunction(next => window.__COLD__.active === next, next);
  }
  await page.evaluate(() => window.__COLD__.release());
  await page.waitForFunction(() => !document.querySelector('.usage-sync-status'));
  assert.equal(await page.locator('.usage-stat strong').first().textContent(), '4,467,458,821');
  await page.getByRole('button', { name: '会话', exact: true }).click();
  assert.ok(await page.evaluate(() => window.__COLD__.calls.some(c => c.cmd === 'cancel_usage')));
  assert.deepEqual(errors, []);
  console.log(`PASS: completed-source results, billion-token totals, long paths, errors/retries, and cancellation stay usable (${engine})`);
} finally { await browser.close(); }
