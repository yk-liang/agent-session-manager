import { engine, baseURL, artifactDir } from "../helpers/browser.mjs";
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const privateDir = mkdtempSync(path.join(tmpdir(), 'asm-usage-fixture-'));
const exported = path.join(privateDir, 'usage.json');
let browser;
try {
 const run = await promisify(execFile)('cargo', ['test', '--offline', '--locked', 'export_usage_summary_fixture_if_requested', '--', '--nocapture'], { cwd: path.resolve('src-tauri'), env: { ...process.env, ASM_USAGE_SUMMARY_EXPORT: exported }, timeout: 180000 });
 assert.match(run.stdout, /1 passed/);
 const fixture = JSON.parse(readFileSync(exported, 'utf8'));
 browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
 const page = await browser.newPage({ viewport: { width: 1280, height: 900 } }), errors = [];
 page.on('pageerror', e => errors.push(e.message));
 await page.addInitScript(fixture => {
  const NativeDate = Date;
  window.Date = class extends NativeDate {
   constructor(...args) { super(...(args.length ? args : [fixture.now])); }
   static now() { return fixture.now; }
  };
  const names = { qoder: 'Qoder CLI', claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', pi: 'Pi' };
  const state = window.__USAGE__ = { fixture, calls: [], syncRounds: 0, holdSync: true, syncResolvers: [], releaseSync: null, holdSummary: null, releaseSummary: null, failSync: null, failSummary: false, collector: { desired: false, running: false, authorization: 'disabled' } };
  window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
   state.calls.push({ cmd, args });
   if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, supportsModelNames: false, supportsFastCatalog: false, dataDir: null, deletePolicy: 'trash' }));
   if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
   if (cmd === 'collector_status') return structuredClone(state.collector);
   if (cmd === 'set_collector_enabled') { state.collector = { desired: args.enabled, running: args.enabled, authorization: args.enabled ? 'enabled' : 'disabled' }; return structuredClone(state.collector); }
   if (cmd === 'sync_request_events') {
    if (state.holdSync) { await new Promise(resolve => { state.syncResolvers.push(resolve); state.releaseSync = () => { state.holdSync = false; state.syncResolvers.splice(0).forEach(r => r()); }; }); }
    if (state.failSync === args.agent) throw Error('Read failed');
    if (args.agent === 'codex') ++state.syncRounds;
    return { updatedAt: Date.now(), checkedFiles: 100, changedFiles: 0, bytesRead: 0 };
   }
   if (cmd === 'read_usage_summary') {
    const count = args.query.bounds.length;
    const period = count === 0 ? 'all' : count === 8 ? 'week' : count === 31 ? 'month' : 'today';
    const result = structuredClone(fixture.summaries[period]);
    if (state.holdSummary === period) { state.holdSummary = null; await new Promise(resolve => state.releaseSummary = resolve); }
    if (state.failSummary) throw Error('Query failed');
    return result;
   }
   if (cmd === 'read_request_events') {
    const q = args.query;
    const events = fixture.events.events.filter(e => (q.from == null || e.time >= q.from) && (q.to == null || e.time < q.to) && (!q.agent || q.agent === e.agent) && (!q.model || q.model === e.model) && (!q.status || q.status === e.status));
    return { ...fixture.events, events, total: events.length, nextCursor: null, reset: false };
   }
   return null;
  } };
 }, fixture);
 await page.goto(baseURL);
 await page.getByRole('button', { name: '用量', exact: true }).waitFor();
 assert.equal(await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'read_usage_summary').length), 0);
 await page.getByRole('button', { name: '用量', exact: true }).click();
 const total = async () => (await page.locator('.usage-stat strong').first().textContent()).replace(/[^0-9]/g, '');
 const waitTotal = n => page.waitForFunction(n => document.querySelector('.usage-stat strong')?.textContent.replace(/[^0-9]/g, '') === n, n);
 await waitTotal('1100');
 await page.getByText('按需同步', { exact: true }).waitFor();
 await page.waitForTimeout(400);
 assert.equal(await page.locator('.usage-sync-status').count(), 0, 'saved results should update quietly');
 assert.equal(await page.getByRole('button', { name: '刷新', exact: true }).count(), 0);
 const syncCalls = await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'sync_request_events').length);
 await page.getByRole('button', { name: '今日', exact: true }).click(); await waitTotal('660');
 assert.equal(await page.locator('.usage-bucket').count(), 24);
 assert.equal(await page.locator('.usage-bucket').nth(2).getAttribute('data-total'), '110');
 assert.equal(await page.locator('.usage-bucket').nth(15).getAttribute('data-total'), '550');
 await page.waitForFunction(() => document.querySelectorAll('.request-table tbody tr').length === 3);
 await page.getByLabel('统计 Agent').selectOption('claude'); await waitTotal('330');
 await page.getByLabel('统计模型').selectOption('claude-fixture'); await waitTotal('330');
 await page.getByLabel('统计 Agent').selectOption('all');
 for (const [label, count] of [['近 7 天', 7], ['近 30 天', 30]]) {
  await page.getByRole('button', { name: label, exact: true }).click(); await waitTotal('1100');
  assert.equal(await page.locator('.usage-bucket').count(), count);
 }
 await page.getByRole('button', { name: '全部', exact: true }).click(); await waitTotal('2090');
 assert.equal(await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'sync_request_events').length), syncCalls);
 assert.equal(await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'read_usage').length), 0);
 await page.evaluate(() => window.__USAGE__.releaseSync());
 const waitSynced = round => page.waitForFunction(round => window.__USAGE__.syncRounds >= round, round);
 await waitSynced(1);
 console.log(`PASS: persisted results render during sync; request-time hourly/daily buckets, filters and table totals agree; filters never reread logs (${engine})`);

 await page.evaluate(() => window.__USAGE__.failSync = 'claude');
 await page.evaluate(() => { window.dispatchEvent(new Event('blur')); window.dispatchEvent(new Event('focus')); });
 await page.locator('.usage-source-error').waitFor(); await waitSynced(2); assert.equal(await total(), '2090');
 await page.evaluate(() => window.__USAGE__.failSync = null);
 await page.getByRole('button', { name: '重试同步', exact: true }).click(); await waitSynced(3);
 await page.waitForFunction(() => !document.querySelector('.usage-source-error'));
 await page.evaluate(() => window.__USAGE__.holdSummary = 'today');
 await page.getByRole('button', { name: '今日', exact: true }).click();
 await page.waitForFunction(() => window.__USAGE__.releaseSummary !== null);
 await page.getByRole('button', { name: '近 7 天', exact: true }).click(); await waitTotal('1100');
 await page.evaluate(() => window.__USAGE__.releaseSummary());
 assert.equal(await page.locator('.usage-bucket').count(), 7); assert.equal(await total(), '1100');
 await page.evaluate(() => window.__USAGE__.failSummary = true);
 await page.getByRole('button', { name: '近 30 天', exact: true }).click();
 await page.locator('.usage-query-error').waitFor(); assert.equal(await total(), '1100');
 await page.evaluate(() => window.__USAGE__.failSummary = false);
 await page.getByRole('button', { name: '重试查询', exact: true }).click();
 await page.waitForFunction(() => !document.querySelector('.usage-query-error'));
 console.log(`PASS: source/query errors preserve saved results; retry recovers; late queries cannot replace the selected range (${engine})`);

 await page.getByRole('button', { name: '开启后台采集', exact: true }).click();
 await page.getByText('后台保留已开启', { exact: true }).waitFor();
 await page.getByRole('button', { name: '暂停后台采集', exact: true }).click();
 await page.getByText('按需同步', { exact: true }).waitFor();
 await page.getByRole('button', { name: '同步与保留说明', exact: true }).click();
 assert.ok((await page.getByRole('tooltip').textContent()).includes('开启后台保留后，退出应用也会继续记录'));
 await page.keyboard.press('Escape');
 await page.getByRole('button', { name: '今日', exact: true }).click(); await waitTotal('660');
 let labelSizes;
 for (const width of [960, 1280, 1600, 2200, 1280]) {
  await page.setViewportSize({ width, height: 900 });
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
  await page.waitForFunction(() => {
   const svg = document.querySelector('.usage-chart svg');
   const grid = svg?.querySelector('.usage-grid-line');
   if (!grid) return false;
   const rightGap = svg.getBoundingClientRect().right - grid.getBoundingClientRect().right;
   return rightGap >= 16 && rightGap <= 32;
  });
  const labels = await page.locator('.usage-chart svg text').evaluateAll(elements => elements.map(el => {
   const box = el.getBoundingClientRect(), svg = el.ownerSVGElement.getBoundingClientRect(), matrix = el.getScreenCTM();
   return { text: el.textContent, width: box.width, height: box.height, scaleX: matrix.a, scaleY: matrix.d,
    contained: box.left >= svg.left - 1 && box.right <= svg.right + 1 && box.top >= svg.top - 1 && box.bottom <= svg.bottom + 1 };
  }));
  for (const label of labels) {
   assert.ok(Math.abs(label.scaleX - 1) < .01 && Math.abs(label.scaleY - 1) < .01, `chart text is scaled at ${width}: ${JSON.stringify(label)}`);
   assert.ok(label.contained, `chart label clips at ${width}: ${label.text}`);
  }
  if (labelSizes) for (const [i, label] of labels.entries()) {
   assert.ok(Math.abs(label.width - labelSizes[i].width) < .5 && Math.abs(label.height - labelSizes[i].height) < .5, `font dimensions changed at ${width}`);
  } else labelSizes = labels;
 }
 await page.setViewportSize({ width: 1280, height: 900 });
 await page.screenshot({ path: path.join(artifactDir, 'usage-hourly.png') });
 await page.getByRole('button', { name: '会话', exact: true }).click();
 assert.ok(await page.evaluate(() => window.__USAGE__.calls.some(c => c.cmd === 'cancel_usage_summary')));
 assert.deepEqual(errors, []);
 console.log(`PASS: background retention stays independent; chart labels keep their size and proportions at 960/1280/1600/2200 and after shrinking (${engine})`);
} finally { await browser?.close(); rmSync(privateDir, { recursive: true, force: true }); }
