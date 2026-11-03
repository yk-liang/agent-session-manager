import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import path from 'node:path';
import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chooseAgent } from '../helpers/agent-switcher.mjs';

const primaryId = 'mode-d26f1923f8-fixture';
const primaryName = 'Qwen3.8-Max-DogFooding';
const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
const errors = [];
page.on('pageerror', error => errors.push(error.message));
try {
  await page.addInitScript(({ primaryId, primaryName }) => {
    Object.defineProperty(document, 'hasFocus', { value: () => false });
    const state = window.__MODEL_NAMES__ = {
      calls: [], hold: true, pending: [], fail: false, syncs: 0, bulk: false,
      registry: { [primaryId]: primaryName, 'mode-failed-only': 'Qwen failed request model', 'mode-history-only': 'Qwen historical request model' },
    };
    state.release = () => { state.hold = false; state.pending.splice(0).forEach(resolve => resolve()); };
    const tokens = {
      inputTokens: 100, outputTokens: 10, cacheReadTokens: 80, cacheWriteTokens: 0, reasoningTokens: 0, unclassifiedTokens: 0,
      inputRecorded: true, outputRecorded: true, cacheReadRecorded: true, cacheWriteRecorded: false, reasoningRecorded: false,
      inputIncomplete: false, outputIncomplete: false, cacheReadIncomplete: false, cacheWriteIncomplete: false, reasoningIncomplete: false,
    };
    const event = (agent, model, i, modelName = model) => ({
      ...tokens, id: `${agent}-${i}`, agent, model, modelName, time: Date.now() - (i + 1) * 1000,
      session: 'fixture-session', status: model === 'mode-failed-only' ? 'failed' : 'success', partial: false,
      request: { client: 'cli', project: '/fixture/qoder', scope: 'request' }, outputTps: null,
    });
    const events = Array.from({ length: 55 }, (_, i) => event('qoder',
      i === 1 ? 'mode-unresolved' : i === 2 ? 'mode-failed-only' : i >= 50 ? 'mode-history-only' : primaryId, i));
    events.push(event('codex', primaryId, 0, 'Codex recorded model'));
    const summaryRow = (agent, model, modelName = model) => ({
      ...tokens, agent, model, modelName, date: '', bucketStart: Date.now() - 1000, totalTokens: 110,
      requestCount: 1, requestCountIncomplete: false, costUsd: null, costIncomplete: false, partial: false,
      cacheRateUnavailable: agent === 'qoder',
    });
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
      state.calls.push({ cmd, args: structuredClone(args) });
      if (cmd === 'list_agents') return [
        { id: 'codex', displayName: 'Codex', installed: true, supportsModelNames: false, deletePolicy: 'trash' },
        { id: 'qoder', displayName: 'Qoder CLI', installed: true, supportsModelNames: true, deletePolicy: 'trash' },
      ];
      if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (cmd === 'collector_status') return { desired: false, running: false, authorization: 'disabled' };
      if (cmd === 'sync_request_events') { state.syncs++; return { updatedAt: Date.now(), checkedFiles: 0, changedFiles: 0, bytesRead: 0 }; }
      if (cmd === 'read_usage_summary') return {
        rows: state.bulk ? Array.from({ length: 270 }, (_, i) => summaryRow('qoder', `mode-bulk-${String(i).padStart(3, '0')}`)) : [
          summaryRow('qoder', primaryId), summaryRow('qoder', 'mode-unresolved'), summaryRow('codex', primaryId, 'Codex recorded model'),
        ], health: {}, syncedAgents: ['qoder', 'codex'], revision: String(state.syncs),
      };
      if (cmd === 'read_request_events') {
        const q = args.query;
        const filtered = events.filter(e => (!q.agent || e.agent === q.agent) && (!q.model || e.model === q.model) && (!q.status || e.status === q.status));
        const pageSize = q.limit ?? 50, anchor = q.anchor ? filtered.findIndex(e => e.id === q.anchor) : -1;
        const number = anchor < 0 ? q.page ?? 1 : Math.floor(anchor / pageSize) + 1;
        const page = Math.max(1, Math.min(number, Math.ceil(filtered.length / pageSize)));
        const start = (page - 1) * pageSize;
        return { events: filtered.slice(start, start + pageSize), total: filtered.length, models: [...new Set(filtered.map(e => e.model))], revision: 'fixed', page, pageSize };
      }
      if (cmd === 'resolve_model_names') {
        if (args.models.length > 256) throw Error('Too many model IDs');
        if (state.hold) await new Promise(resolve => state.pending.push(resolve));
        if (state.fail) throw Error('Model registry unavailable');
        return Object.fromEntries(args.models.flatMap(id => {
          const name = id.startsWith('mode-bulk-') ? `Readable ${id}` : state.registry[id];
          return name ? [[id, name]] : [];
        }));
      }
      return null;
    } };
  }, { primaryId, primaryName });

  await page.goto(baseURL);
  await page.getByRole('button', { name: '用量', exact: true }).click();
  await chooseAgent(page, 'qoder');
  await page.getByText('第 1–50 条，共 55 条', { exact: true }).waitFor();
  await page.waitForFunction(() => window.__MODEL_NAMES__.pending.length > 0);
  const table = page.locator('.request-events');
  const rowLabel = id => table.locator(`.request-model .model-label[title="${id}"], .request-model .model-label[title$="模型 ID：${id}"]`).first();
  const stats = await page.locator('.usage-stat strong').allTextContents();
  assert.equal(await rowLabel(primaryId).textContent(), primaryId);
  assert.ok(stats.every(value => value !== '—'), 'token summaries must load before model names');
  assert.equal(await page.evaluate(() => window.__MODEL_NAMES__.calls.filter(c => c.cmd === 'catalog' && c.args.agent === 'qoder').length), 0);
  console.log(`PASS: usage opens without visiting Qoder sessions and does not wait for model resolution (${engine})`);

  // A late response may enrich Qoder, but the same ID in another Agent keeps its own label.
  await chooseAgent(page, 'codex');
  await page.getByText('第 1–1 条，共 1 条', { exact: true }).waitFor();
  await page.evaluate(() => window.__MODEL_NAMES__.release());
  await page.waitForFunction(() => window.__MODEL_NAMES__.pending.length === 0);
  assert.equal(await rowLabel(primaryId).textContent(), 'Codex recorded model');
  await chooseAgent(page, 'qoder');
  await rowLabel(primaryId).filter({ hasText: primaryName }).waitFor();
  await page.locator('.usage-models .model-label').filter({ hasText: primaryName }).waitFor();
  assert.equal(await page.getByLabel('统计模型', { exact: true }).locator(`option[value="${primaryId}"]`).textContent(), primaryName);
  assert.match(await rowLabel(primaryId).getAttribute('title'), /模型 ID：mode-d26/);
  assert.equal(await rowLabel('mode-unresolved').textContent(), 'mode-unresolved');
  assert.deepEqual(await page.locator('.usage-stat strong').allTextContents(), stats, 'names must not alter token totals');
  await rowLabel('mode-failed-only').filter({ hasText: 'Qwen failed request model' }).waitFor();
  console.log(`PASS: request rows, model filters and summary labels agree; raw IDs, totals and Agent boundaries are preserved (${engine})`);

  await page.getByLabel('统计模型', { exact: true }).selectOption(primaryId);
  await page.waitForFunction(id => window.__MODEL_NAMES__.calls.some(c => c.cmd === 'read_request_events' && c.args.query.model === id), primaryId);
  assert.equal(await page.getByLabel('统计模型', { exact: true }).inputValue(), primaryId);
  await page.getByLabel('统计模型', { exact: true }).selectOption('all');
  await page.getByText('第 1–50 条，共 55 条', { exact: true }).waitFor();
  await table.getByRole('button', { name: '下一页', exact: true }).click();
  await page.getByText('第 51–55 条，共 55 条', { exact: true }).waitFor();
  await rowLabel('mode-history-only').filter({ hasText: 'Qwen historical request model' }).waitFor();
  console.log(`PASS: failed-only and later-page models resolve even when absent from the usage summary (${engine})`);

  await page.screenshot({ path: path.join(artifactDir, 'usage-model-names.png'), fullPage: true });
  // The next mount starts with raw persisted event/summary data, just as an app restart does.
  await page.evaluate(() => { window.__MODEL_NAMES__.fail = true; });
  await page.getByRole('button', { name: '会话', exact: true }).click();
  const previousCalls = await page.evaluate(() => window.__MODEL_NAMES__.calls.filter(c => c.cmd === 'resolve_model_names').length);
  await page.getByRole('button', { name: '用量', exact: true }).click();
  await page.getByText('第 1–50 条，共 55 条', { exact: true }).waitFor();
  await page.waitForFunction(n => window.__MODEL_NAMES__.calls.filter(c => c.cmd === 'resolve_model_names').length > n, previousCalls);
  assert.equal(await rowLabel(primaryId).textContent(), primaryId);
  assert.equal(await page.getByRole('alert').count(), 0);
  const beforeRetry = await page.evaluate(() => window.__MODEL_NAMES__.syncs);
  await page.evaluate(() => { window.__MODEL_NAMES__.fail = false; window.dispatchEvent(new Event('focus')); });
  await page.waitForFunction(n => window.__MODEL_NAMES__.syncs > n, beforeRetry);
  await rowLabel(primaryId).filter({ hasText: primaryName }).waitFor();
  console.log(`PASS: unavailable names leave readable data in place and retry on the next automatic sync (${engine})`);

  await page.getByRole('button', { name: '会话', exact: true }).click();
  await page.evaluate(() => { window.__MODEL_NAMES__.bulk = true; });
  await page.getByRole('button', { name: '用量', exact: true }).click();
  const lastId = 'mode-bulk-269';
  await page.waitForFunction(id => document.querySelector(`select[aria-label="统计模型"] option[value="${id}"]`)?.textContent === `Readable ${id}`, lastId);
  assert.ok(await page.evaluate(() => window.__MODEL_NAMES__.calls.filter(c => c.cmd === 'resolve_model_names').every(c => c.args.models.length <= 256)));
  assert.equal(await page.getByRole('alert').count(), 0);
  assert.deepEqual(errors, []);
  console.log(`PASS: more than 256 historical models resolve within the backend batch limit (${engine})`);
} finally {
  await browser.close();
}
