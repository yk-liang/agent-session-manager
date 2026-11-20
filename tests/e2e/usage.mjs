import { addUsageInitScript } from '../helpers/usage-collector.mjs';
import { chooseOption } from '../helpers/dropdown.mjs';
import { chooseAgent } from '../helpers/agent-switcher.mjs';
import { assertTooltipValues } from '../helpers/tooltip.mjs';
import { assertTripleClickSelects } from '../helpers/selection.mjs';
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
 const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, deviceScaleFactor: 2 }), errors = [];
 page.on('pageerror', e => errors.push(e.message));
 await addUsageInitScript(page, (fixture, createCollector) => {
  const NativeDate = Date;
  window.Date = class extends NativeDate {
   constructor(...args) { super(...(args.length ? args : [fixture.now])); }
   static now() { return fixture.now; }
  };
  const names = { qoder: 'Qoder', claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', pi: 'Pi' };
  const state = window.__USAGE__ = {
   fixture, calls: [], syncRounds: 0, holdSync: true, syncResolvers: [], releaseSync: null, holdSummary: null, releaseSummary: null, failSync: null, failSummary: false,
   pricingRevision: 1,
   customPrices: {},
   failLitellmRefresh: false,
   holdRefSearch: false,
   refSearchResolvers: [],
   sources: [
    { id: 'models.dev', name: 'Models.dev', url: 'https://models.dev/api.json', fetchedAt: fixture.now - 3600000, modelCount: 120, pricedCount: 100, stale: false, error: null },
    { id: 'litellm', name: 'LiteLLM', url: 'https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json', fetchedAt: fixture.now - 3600000, modelCount: 200, pricedCount: 160, stale: false, error: null },
   ],
  };
  const callbacks = new Map(), listeners = new Map(); let nextId = 1;
  const emit = (event, payload) => { for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ event, id, payload }); };
  const resolvePrice = model => {
   if (model === 'local/demo:free') return { source: 'builtin', currency: 'USD', rates: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } };
   const custom = state.customPrices[model];
   if (custom) {
    const scale = custom.currency === 'CNY' ? 1 / 7 : 1;
    const long = custom.longContext;
    const rates = { longContext: long ? { ...long, input: long.input * scale, output: long.output * scale } : null, input: custom.input * scale, output: custom.output * scale, cacheRead: custom.cacheRead == null ? null : custom.cacheRead * scale, cacheWrite: custom.cacheWrite == null ? null : custom.cacheWrite * scale };
    return { source: 'custom', currency: custom.currency, originalRates: { input: custom.input, output: custom.output, cacheRead: custom.cacheRead, cacheWrite: custom.cacheWrite, longContext: long }, rates, overridesReference: model === 'gpt-fixture' ? { source: 'models.dev', provider: 'openai', modelId: 'gpt-fixture', rates: { input: 2, output: 8, cacheRead: 0.2, cacheWrite: null } } : null };
   }
   if (model === 'gpt-fixture') return { source: 'models.dev', provider: 'openai', modelId: 'gpt-fixture', currency: 'USD', rates: { input: 2, output: 8, cacheRead: 0.2, cacheWrite: null } };
   return null;
  };
  const priceEvent = e => {
   const p = resolvePrice(e.model);
   if (!p || !e.inputRecorded || !e.outputRecorded) return null;
   const uncached = Math.max(0, e.inputTokens - e.cacheReadTokens - e.cacheWriteTokens);
   const r = p.rates;
   const totalUsd = (uncached * r.input + e.cacheReadTokens * (r.cacheRead ?? r.input) + e.cacheWriteTokens * (r.cacheWrite ?? r.input) + e.outputTokens * r.output) / 1e6;
   return { totalUsd, source: p.source, currency: p.currency, inputRate: r.input, outputRate: r.output, cacheReadRate: r.cacheRead ?? r.input, cacheWriteRate: r.cacheWrite ?? r.input };
  };
  state.collector = createCollector(['qoder', 'claude', 'opencode', 'pi', 'codex'], async agent => {
   if (state.holdSync) await new Promise(resolve => {
    state.syncResolvers.push(resolve);
    state.releaseSync = () => { state.holdSync = false; state.syncResolvers.splice(0).forEach(fn => fn()); };
   });
   if (state.failSync === agent) throw Error('Read failed');
   if (agent === 'codex') ++state.syncRounds;
   return false;
  }, handler => callbacks.get(handler));
  window.__TAURI_INTERNALS__ = {
   transformCallback: cb => { const id = nextId++; callbacks.set(id, cb); return id; },
   unregisterCallback: id => callbacks.delete(id),
   invoke: state.collector.bridge(async (cmd, args = {}) => {
    state.calls.push({ cmd, args });
    if (cmd === 'plugin:event|listen') { const set = listeners.get(args.event) ?? new Set(); set.add(args.handler); listeners.set(args.event, set); return args.handler; }
    if (cmd === 'plugin:event|unlisten') { listeners.get(args.event)?.delete(args.eventId); return null; }
   if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, dataDir: null, deletePolicy: 'trash' }));
   if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
   if (cmd === 'read_usage_summary') {
    const count = args.query.bounds.length;
    const period = count === 0 ? 'all' : count === 8 ? 'week' : count === 31 ? 'month' : 'today';
    const result = structuredClone(fixture.summaries[period]);
    result.syncedAgents = Object.keys(names); // This fixture represents a previously completed import.
    result.pricingRevision = state.pricingRevision;
    // Apply custom prices to fixture rows when set during pricing checks.
    for (const row of result.rows) {
     const custom = state.customPrices[row.model];
     if (custom) {
      const scale = custom.currency === 'CNY' ? 1 / 7 : 1;
      const uncached = Math.max(0, row.inputTokens - row.cacheReadTokens - row.cacheWriteTokens);
      const est = (uncached * custom.input * scale + row.cacheReadTokens * (custom.cacheRead ?? custom.input) * scale + row.cacheWriteTokens * (custom.cacheWrite ?? custom.input) * scale + row.outputTokens * custom.output * scale) / 1e6;
      row.costUsd = est; row.customCostUsd = est; row.referenceCostUsd = null; row.costUnknownCount = 0; row.unpricedCount = 0; row.costIncomplete = false;
     }
    }
    if (state.holdSummary === period) { state.holdSummary = null; await new Promise(resolve => state.releaseSummary = resolve); }
    if (state.failSummary) throw Error('Query failed');
    return result;
   }
   if (cmd === 'read_request_events') {
    const q = args.query;
    const events = fixture.events.events.filter(e => (q.from == null || e.time >= q.from) && (q.to == null || e.time < q.to) && (!q.agent || q.agent === e.agent) && (!q.model || q.model === e.model) && (!q.status || q.status === e.status)).map(e => ({ ...e, cost: priceEvent(e) }));
    return { ...fixture.events, events, total: events.length, hasCacheWrites: events.some(event => event.cacheWriteTokens > 0),
     hasServiceTiers: events.some(event => event.request.tier?.trim()),
     page: 1, pageSize: args.query.limit ?? 50 };
   }
   if (cmd === 'get_model_prices') {
    const byModel = new Map();
    for (const e of fixture.events.events) {
     if (args.agent && e.agent !== args.agent) continue;
     const row = byModel.get(e.model) ?? { model: e.model, agents: [], providers: [], requests: 0, totalTokens: 0, customOnly: false };
     if (!row.agents.includes(e.agent)) row.agents.push(e.agent);
     row.requests++; row.totalTokens += e.inputTokens + e.outputTokens;
     byModel.set(e.model, row);
    }
    for (const model of Object.keys(state.customPrices)) {
     if (!byModel.has(model)) byModel.set(model, { model, agents: [], providers: [], requests: 0, totalTokens: 0, customOnly: true });
    }
    if (!args.agent || args.agent === 'opencode') {
     byModel.set('local/demo:free', { model: 'local/demo:free', agents: ['opencode'], providers: ['local'], requests: 1, totalTokens: 100, customOnly: false });
    }
    return {
     models: [...byModel.values()].map(r => ({ ...r, price: resolvePrice(r.model) })),
     catalog: { revision: state.pricingRevision, sources: structuredClone(state.sources), customCount: Object.keys(state.customPrices).length, exchangeRateCnyPerUsd: 7 },
    };
   }
   if (cmd === 'set_custom_price') {
    state.customPrices[args.input.model] = structuredClone(args.input);
    state.pricingRevision++;
    const status = { revision: state.pricingRevision, sources: structuredClone(state.sources), customCount: Object.keys(state.customPrices).length, exchangeRateCnyPerUsd: 7 };
    emit('usage-pricing-updated', status.revision);
    return status;
   }
   if (cmd === 'delete_custom_price') {
    delete state.customPrices[args.model];
    state.pricingRevision++;
    const status = { revision: state.pricingRevision, sources: structuredClone(state.sources), customCount: Object.keys(state.customPrices).length, exchangeRateCnyPerUsd: 7 };
    emit('usage-pricing-updated', status.revision);
    return status;
   }
   if (cmd === 'refresh_reference_prices') {
    state.sources[0].fetchedAt = fixture.now; state.sources[0].error = null;
    if (state.failLitellmRefresh) { state.sources[1].error = 'network timeout'; } else { state.sources[1].fetchedAt = fixture.now; state.sources[1].error = null; }
    state.pricingRevision++;
    const status = { revision: state.pricingRevision, sources: structuredClone(state.sources), customCount: Object.keys(state.customPrices).length, exchangeRateCnyPerUsd: 7 };
    emit('usage-pricing-updated', status.revision);
    return status;
   }
   if (cmd === 'search_reference_prices') {
    if (state.holdRefSearch) {
     await new Promise(resolve => state.refSearchResolvers.push(resolve));
    }
    return [{ source: 'models.dev', provider: 'openai', modelId: 'gpt-fixture', rates: { input: 2, output: 8, cacheRead: 0.2, cacheWrite: null } }];
   }
   return null;
  }) };
  state.collector.changed();
  void state.collector.check();
 }, fixture);
 await page.goto(baseURL);
 await page.getByRole('button', { name: '用量', exact: true }).waitFor();
 assert.equal(await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'read_usage_summary').length), 0);
 await page.getByRole('button', { name: '用量', exact: true }).click();
 const total = async () => await page.locator('.usage-stat[data-metric="tokens"] strong').getAttribute('data-value');
 const waitTotal = n => page.waitForFunction(n => document.querySelector('.usage-stat[data-metric="tokens"] strong')?.dataset.value === n, n);
 const selectAgent = name => chooseAgent(page, { label: name });
 // 统计分析为默认入口子页。
 assert.equal(await page.getByRole('tab', { name: '统计分析', exact: true }).getAttribute('aria-selected'), 'true');
 await waitTotal('1100');
 const cardMeta = metric => page.locator(`.usage-stat[data-metric="${metric}"] .usage-stat-meta > div`);
 const cardLabels = metric => cardMeta(metric).locator(':scope > span').allTextContents();
 const cardValues = metric => cardMeta(metric).locator('b').allTextContents();
 assert.deepEqual(await page.locator('.usage-stat > span').allTextContents(), ['请求总数', 'Token 用量', '缓存命中率', '总费用']);
 assert.deepEqual(await cardLabels('requests'), ['成功 / 失败', '成功率']);
 assert.deepEqual(await cardValues('requests'), ['4 / 0', '100%']);
 assert.deepEqual(await cardLabels('tokens'), ['输入', '输出']);
 assert.deepEqual(await cardValues('tokens'), ['1K', '100']);
 assert.deepEqual(await cardLabels('cache'), ['缓存读取', '未缓存输入']);
 assert.deepEqual(await cardValues('cache'), ['450', '550']);
 assert.deepEqual(await cardLabels('cost'), ['模型第一', '项目第一']);
 assert.deepEqual(await cardValues('cost'), ['—', '—']);
 assert.equal(await page.locator('.usage-details,.usage-agent-tabs').count(), 0);
 const modelCells = selector => page.locator(selector).locator('td').allTextContents();
 const modelRow = name => `.usage-models tbody tr:has(.model-label:text-is("${name}"))`;
 assert.deepEqual(await page.locator('.usage-models thead th').allTextContents(), ['模型','请求','成功率','输入','输出','缓存读取','缓存命中率','费用（USD）']);
 assert.deepEqual(await modelCells(modelRow('gpt-fixture')), ['2','100%','700','70','450','64.3%','—']);
 assert.deepEqual(await modelCells('.usage-models tfoot tr'), ['4','100%','1K','100','450','45.0%','—']);
 await page.locator('.usage-models tbody .usage-num').first().dblclick();
 assert.ok(await page.evaluate(() => getSelection().toString()), 'summary cells are selectable');
 await page.evaluate(() => getSelection().removeAllRanges());
 assert.equal(await page.getByRole('combobox', { name: '请求 Agent', exact: true }).count(), 0);
 assert.equal(await page.getByRole('combobox', { name: '请求模型', exact: true }).count(), 0);
 assert.ok(await page.evaluate(() => document.querySelector('.agent-switcher').getBoundingClientRect().bottom < document.querySelector('.usage-cards').getBoundingClientRect().top));
 await page.locator('.usage-page').evaluate(el => { el.scrollTop = 40; });
 const scrollBefore = await page.locator('.usage-page').evaluate(el => el.scrollTop);
 assert.ok(scrollBefore > 0);
 for (const [name, amount] of [['Codex','770'], ['Pi','0'], ['全部 Agent','1100']]) {
  await selectAgent(name);
  await waitTotal(amount);
  if (name === 'Pi') {
   assert.ok(await page.locator('.usage-page').evaluate(el => el.scrollTop <= el.scrollHeight - el.clientHeight + 1));
  } else if (name === 'Codex') {
   assert.ok(Math.abs(await page.locator('.usage-page').evaluate(el => el.scrollTop) - scrollBefore) < 2, `Agent switch moved the page: ${name}`);
  }
 }
 assert.equal(await page.getByText('按需同步', { exact: true }).count(), 0);
 await page.waitForTimeout(400);
 assert.equal(await page.locator('.usage-coverage').count(), 0, 'saved results should update quietly');
 assert.equal(await page.getByRole('button', { name: '刷新', exact: true }).count(), 0);
 const syncCalls = await page.evaluate(() => window.__USAGE__.collector.scans.length);
 await page.getByRole('button', { name: '今日', exact: true }).click(); await waitTotal('660');
 // Local days can have 23 or 25 hours when daylight saving time changes.
 const hoursToday = await page.evaluate(() => {
  const now = new Date();
  return Math.ceil((new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1) - new Date(now.getFullYear(), now.getMonth(), now.getDate())) / 3600000);
 });
 assert.equal(await page.locator('.usage-bucket').count(), hoursToday);
 assert.equal(await page.locator('.usage-bucket').nth(2).getAttribute('data-total'), '110');
 assert.equal(await page.locator('.usage-bucket').nth(15).getAttribute('data-total'), '550');
 const chartTip = page.getByRole('tooltip');
 for (const [index, total] of [[2, '110'], [15, '550']]) {
  await page.locator('.usage-bucket').nth(index).hover();
  await chartTip.getByText(total, { exact: true }).waitFor();
  await assertTooltipValues(chartTip);
 }
 assert.equal(await page.locator('.usage-chart svg title').count(), 0);
 await page.keyboard.press('Escape');
 await chartTip.waitFor({ state: 'hidden' });
 await page.locator('.usage-bucket').nth(2).hover();
 await chartTip.getByText('110', { exact: true }).waitFor();
 await assertTooltipValues(chartTip);
 await page.keyboard.press('Escape');
 // 切换到请求记录子页核对同一时段，再切回统计分析，不触发重新扫描日志。
 const layoutBeforeTab = await page.locator('.usage-toolbar').boundingBox();
 await page.getByRole('tab', { name: '请求记录', exact: true }).click();
 await page.waitForFunction(() => document.querySelectorAll('.request-table tbody tr').length === 3);
 const layoutAfterTab = await page.locator('.usage-toolbar').boundingBox();
 assert.ok(Math.abs(layoutBeforeTab.x - layoutAfterTab.x) < 1);
 assert.ok(Math.abs(layoutBeforeTab.width - layoutAfterTab.width) < 1);
 await page.getByRole('tab', { name: '统计分析', exact: true }).click();
 await waitTotal('660');
 assert.equal(await page.evaluate(() => window.__USAGE__.collector.scans.length), syncCalls);
 await selectAgent('Claude Code'); await waitTotal('330');
 assert.deepEqual(await modelCells('.usage-models tfoot tr'), ['2','100%','300','30','0','0.0%','—']);
 assert.equal(await page.getByRole('combobox', { name: '模型', exact: true }).count(), 0);
 await selectAgent('Claude Code');
 assert.equal(await page.getByRole('combobox', { name: '当前 Agent', exact: true }).getAttribute("value"), 'claude');
 await selectAgent('全部 Agent');
 for (const [label, count] of [['近 7 天', 7], ['近 30 天', 30]]) {
  await page.getByRole('button', { name: label, exact: true }).click(); await waitTotal('1100');
  assert.equal(await page.locator('.usage-bucket').count(), count);
 }
 await page.getByRole('button', { name: '全部', exact: true }).click(); await waitTotal('2090');
 assert.deepEqual(await modelCells('.usage-models tfoot tr'), ['5','100%','1.9K','190','450','23.7%','—']);
 assert.equal(await page.evaluate(() => window.__USAGE__.collector.scans.length), syncCalls);
 assert.equal(await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'read_usage').length), 0);
 await page.evaluate(() => window.__USAGE__.releaseSync());
 const waitSynced = round => page.waitForFunction(round => window.__USAGE__.syncRounds >= round, round);
 await waitSynced(1);
 // A confirmed empty baseline remains zero during subsequent refreshes.
 await selectAgent('Pi'); await waitTotal('0');
 await selectAgent('全部 Agent'); await waitTotal('2090');
 console.log(`PASS: persisted results render during sync; request-time hourly/daily buckets, filters and table totals agree; filters never reread logs (${engine})`);

 await page.evaluate(() => window.__USAGE__.failSync = 'claude');
 await page.evaluate(() => { void window.__USAGE__.collector.check(); });
 const syncFailure = page.getByRole('button', { name: '同步状态，同步失败' });
 await syncFailure.waitFor(); await waitSynced(2); assert.equal(await total(), '2090');
 await page.evaluate(() => window.__USAGE__.failSync = null);
 await syncFailure.click();
 await page.getByRole('button', { name: '重试同步', exact: true }).click(); await waitSynced(3);
 await syncFailure.waitFor({ state: 'hidden' });
 await page.keyboard.press('Escape');
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
 await page.locator('.usage-query-error').getByRole('button', { name: '重试', exact: true }).click();
 await page.waitForFunction(() => !document.querySelector('.usage-query-error'));
 console.log(`PASS: source/query errors preserve saved results; retry recovers; late queries cannot replace the selected range (${engine})`);

 const syncChip = page.locator('.usage-sync-trigger');
 await syncChip.filter({ hasText: /\d{2}:\d{2}:\d{2} 更新/ }).waitFor();
 const syncDot = syncChip.locator('.usage-sync-dot');
 const syncBurst = syncDot.locator('.usage-sync-pulse');
 const pulseAnimation = () => syncDot.evaluate(dot => getComputedStyle(dot, '::before').animationName);
 assert.equal(await pulseAnimation(), 'usage-sync-ping');
 assert.equal(await syncBurst.count(), 0);
 const updateUsageTimestamp = () => page.evaluate(() => {
  const { fixture, collector } = window.__USAGE__;
  fixture.now += 1000;
  collector.changed();
 });
 await updateUsageTimestamp();
 await syncBurst.waitFor({ state: 'attached' });
 assert.equal(await syncBurst.evaluate(ring => getComputedStyle(ring).animationName), 'usage-sync-burst');
 await syncBurst.waitFor({ state: 'detached' });
 await page.emulateMedia({ reducedMotion: 'reduce' });
 assert.equal(await pulseAnimation(), 'none');
 const previousSyncLabel = await syncChip.textContent();
 await updateUsageTimestamp();
 await syncChip.filter({ hasNotText: previousSyncLabel }).waitFor();
 assert.equal(await syncBurst.count(), 0);
 await page.emulateMedia({ reducedMotion: 'no-preference' });
 await syncChip.click();
 await page.getByRole('dialog', { name: '用量同步状态' }).locator('time').waitFor();
 assert.equal(await page.getByText('后台采集未开启', { exact: true }).count(), 0);
 await page.getByRole('button', { name: '立即检查', exact: true }).waitFor();
 await page.keyboard.press('Escape');
 await page.getByRole('button', { name: '今日', exact: true }).click(); await waitTotal('660');
 let labelSizes;
 for (const width of [960, 1280, 1600, 2200, 1280]) {
  await page.setViewportSize({ width, height: 900 });
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
  const tableLayout = await page.locator('.usage-models').evaluate(el => ({
   width: el.clientWidth, scrollWidth: el.scrollWidth,
   overflowingCells: [...el.querySelectorAll('td')].filter(cell => cell.scrollWidth > cell.clientWidth + 1).map(cell => cell.textContent),
  }));
  assert.ok(tableLayout.scrollWidth <= tableLayout.width + 1, `model section overflows at ${width}`);
  assert.deepEqual(tableLayout.overflowingCells, [], `model values overlap at ${width}`);
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
 await page.locator('.usage-page').evaluate(el => { el.scrollTop = 0; });
 await page.screenshot({ path: path.join(artifactDir, 'usage-agent-switcher.png') });
 await page.locator('.usage-models').screenshot({ path: path.join(artifactDir, 'usage-models.png') });
 // More than eight models: the footer always includes hidden rows, with large values and long names.
 await page.evaluate(() => {
  const summary = window.__USAGE__.fixture.summaries.today, base = summary.rows[0];
  summary.rows = Array.from({ length: 10 }, (_, i) => ({ ...base,
   model: `provider/model-${i + 1}-with-a-long-name-for-layout-check`,
   inputTokens: (i + 1) * 100_000_000, outputTokens: (i + 1) * 10_000, totalTokens: (i + 1) * 100_010_000,
   cacheReadTokens: (i + 1) * 90_000_000, cacheWriteTokens: 10, cacheWriteRecorded: true,
   requestCount: (i + 1) * 100, successCount: (i + 1) * 90, failedCount: (i + 1) * 10,
   costUsd: i === 0 ? 0.000001 : (i + 1) * 0.5,
   referenceCostUsd: i === 0 ? 0.000001 : (i + 1) * 0.5, costIncomplete: i === 9,
  }));
  summary.projects = [
   { ...base, agent: 'claude', key: '/workspace/core/long-project-name-for-layout-check', costUsd: 2 },
   { ...base, agent: 'codex', key: '/workspace/core/long-project-name-for-layout-check', costUsd: 3.25 },
   { ...base, agent: 'codex', key: '/workspace/another-project', costUsd: 4 },
   { ...base, agent: 'codex', key: '', costUsd: 9 },
  ];
 });
 await page.getByRole('button', { name: '近 7 天', exact: true }).click(); await waitTotal('1100');
 await page.getByRole('button', { name: '今日', exact: true }).click(); await waitTotal('5500550000');
 assert.equal(await page.locator('.usage-models tbody tr').count(), 8);
 const allModelsTotal = ['5,500 / 550','90.00%','5.5B','550K','4.95B','90.0%','≥ $27.00'];
 assert.deepEqual(await cardValues('requests'), ['4,950 / 550', '90.00%']);
 assert.equal((await cardLabels('cache'))[0], '缓存读取 / 写入');
 assert.equal((await cardValues('cache'))[0], '4.95B / 100');
 assert.deepEqual(await cardMeta('cost').locator('.usage-stat-value > span').allTextContents(), [
  'provider/model-10-with-a-long-name-for-layout-check', '$5.00',
  'long-project-name-for-layout-check', '$5.25',
 ]);
 assert.deepEqual(await modelCells('.usage-models tfoot tr'), allModelsTotal);
 await page.getByRole('button', { name: '显示全部 10 项', exact: true }).click();
 assert.equal(await page.locator('.usage-models tbody tr').count(), 10);
 assert.deepEqual(await modelCells('.usage-models tfoot tr'), allModelsTotal);
 for (const width of [960, 1280, 2200]) {
  await page.setViewportSize({ width, height: 900 });
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
  const overflow = await page.locator('.usage-models td').evaluateAll(cells => cells.filter(c => c.scrollWidth > c.clientWidth + 1).map(c => c.textContent));
  assert.deepEqual(overflow, [], `large model values overlap at ${width}`);
  assert.ok(await cardMeta('cost').evaluateAll(rows => rows.every(row =>
   getComputedStyle(row, '::before').content === 'none'
   && Math.abs(row.querySelector('.usage-stat-value > span:last-child').getBoundingClientRect().right
    - row.getBoundingClientRect().right) < 1)),
   `cost amounts align without leaders at ${width}`);
 }
 await page.setViewportSize({ width: 1280, height: 900 });
 await page.locator('.usage-models').screenshot({ path: path.join(artifactDir, 'usage-models-large.png') });
 const dimensions = page.getByRole('group', { name: '分组维度' });
 await dimensions.getByRole('button', { name: '按项目', exact: true }).click();
 await assertTripleClickSelects(page.locator('.usage-dim-project').filter({ hasText: 'another-project' }).locator('small'));
 await dimensions.getByRole('button', { name: '按模型', exact: true }).click();
 await page.evaluate(() => {
  const summary = window.__USAGE__.fixture.summaries.today;
  const inputTokens = 141_906_000, outputTokens = 734_000, cacheReadTokens = 138_850_000;
  summary.rows = [{ ...summary.rows[0], model: `provider/${'very-long-model-id-'.repeat(10)}v1`,
   inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens: 0,
   totalTokens: inputTokens + outputTokens }];
  window.__USAGE__.collector.changed();
 });
 await waitTotal('142640000');
 const populatedBucket = page.locator('.usage-bucket[data-total="142640000"]');
 for (const width of [960, 1600]) {
  await page.setViewportSize({ width, height: 900 });
  await page.mouse.move(0, 0);
  await populatedBucket.hover();
  await assertTooltipValues(chartTip, ['734K', '138.85M', '3.06M', '142.64M']);
  assert.equal(await chartTip.locator('.usage-tooltip-note').count(), 0);
 }
 const trendGrouping = page.getByRole('group', { name: '趋势分组' });
 await trendGrouping.getByRole('button', { name: '按模型' }).click();
 await populatedBucket.hover();
 await assertTooltipValues(chartTip, ['142.64M', '142.64M']);
 const longLabel = chartTip.locator('dt.usage-tooltip-label');
 assert.ok(await longLabel.evaluate(label => {
  const style = getComputedStyle(label);
  return style.textOverflow === 'ellipsis' && style.whiteSpace === 'nowrap' && label.scrollWidth > label.clientWidth;
 }));
 await trendGrouping.getByRole('button', { name: '按类别' }).click();
 // 两组截图数据覆盖首次悬浮和切换不同长度的数值。
 await page.evaluate(() => {
  const summary = window.__USAGE__.fixture.summaries.today, row = summary.rows[0];
  const bucket = (bucketStart, successCount, failedCount) => ({ ...row, model: 'gpt-fixture', bucketStart,
   inputTokens: successCount + failedCount, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, totalTokens: successCount + failedCount,
   requestCount: successCount + failedCount, successCount, failedCount, costUsd: null, referenceCostUsd: null });
  summary.rows = [bucket(row.bucketStart, 1694, 3), bucket(row.bucketStart + 3_600_000, 1139, 0)];
  window.__USAGE__.collector.changed();
 });
 await waitTotal('2836');
 await page.getByRole('group', { name: '趋势指标' }).getByRole('button', { name: '请求', exact: true }).click();
 const requestBuckets = { busy: page.locator('.usage-bucket[data-total="1697"]'), quiet: page.locator('.usage-bucket[data-total="1139"]') };
 const requestValues = { busy: ['3', '1,694', '1,697'], quiet: ['1,139', '1,139'] };
 const waitTip = values => page.waitForFunction(text => [...document.querySelectorAll('[role="tooltip"] dd')].map(dd => dd.textContent).join('|') === text, values.join('|'));
 for (const width of [960, 1600]) {
  await page.setViewportSize({ width, height: 900 });
  await page.mouse.move(0, 0);
  await chartTip.waitFor({ state: 'hidden' });
  for (const key of ['busy', 'quiet', 'busy']) {
   await requestBuckets[key].hover();
   await waitTip(requestValues[key]);
   await assertTooltipValues(chartTip, requestValues[key]);
  }
  assert.equal(await chartTip.locator('.usage-tooltip-note').count(), 0);
 }
 const requestPlot = page.getByRole('slider', { name: /每小时请求趋势/ });
 await requestPlot.focus();
 await page.keyboard.press('ArrowRight');
 await waitTip(requestValues.quiet);
 await assertTooltipValues(chartTip, requestValues.quiet);
 await page.keyboard.press('Escape');
 await chartTip.locator('dd').first().waitFor({ state: 'detached' });
 await page.getByRole('group', { name: '趋势指标' }).getByRole('button', { name: 'Token', exact: true }).click();
 await page.setViewportSize({ width: 1280, height: 900 });
 const syncBeforePrices = await page.evaluate(() => window.__USAGE__.collector.scans.length);
 await page.getByRole('button', { name: '近 7 天', exact: true }).click(); await waitTotal('1100');
 await page.getByRole('tab', { name: '模型价格', exact: true }).click();
 const pricesTable = page.locator('.prices-table');
 await pricesTable.waitFor();
 const priceReads = () => page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'get_model_prices').length);
 const initialReads = await priceReads();
 const tableNode = await pricesTable.elementHandle();
 await page.getByRole('tab', { name: '统计分析', exact: true }).click();
 await pricesTable.waitFor({ state: 'hidden' });
 await page.getByRole('tab', { name: '模型价格', exact: true }).click();
 await pricesTable.waitFor();
 assert.equal(await priceReads(), initialReads, 'returning to the same price scope reuses the loaded prices');
 assert.ok(await tableNode.evaluate(node => node.isConnected), 'switching tabs keeps the price table mounted');
 await tableNode.dispose();
 const pricingHelp = page.getByRole('button', { name: '定价规则说明' });
 await pricingHelp.hover();
 await page.getByRole('tooltip').getByText('报价依次采用自定义价格、内置价格、Models.dev 参考价和 LiteLLM 参考价。', { exact: true }).waitFor();
 await page.keyboard.press('Escape');
 const freePriceRow = pricesTable.getByRole('row').filter({ hasText: 'local/demo:free' });
 assert.equal(await freePriceRow.locator('.price-tag').textContent(), '内置');
 assert.equal(await freePriceRow.getByText('$0', { exact: true }).count(), 4);
 await freePriceRow.getByRole('button', { name: '设置价格', exact: true }).click();
 assert.equal(await page.getByLabel('输入单价', { exact: true }).inputValue(), '0');
 await page.getByRole('button', { name: '取消', exact: true }).click();
 await pricesTable.locator('tbody td.numeric > span').first().dblclick({ position: { x: 3, y: 6 } });
 assert.ok(await page.evaluate(() => getSelection().toString()), 'model price cells are selectable');
 await page.evaluate(() => getSelection().removeAllRanges());
 assert.ok((await page.locator('.prices-catalog-bar').textContent()).includes('Models.dev'));
 assert.ok((await page.locator('.prices-catalog-bar').textContent()).includes('LiteLLM'));
 const claudeRow = page.locator('.prices-table tbody tr', { hasText: 'claude-fixture' });
 await claudeRow.getByRole('button', { name: '设置价格', exact: true }).click();
 // 参考价搜索：修改币种/金额不重发检索，清空关键词后忽略晚到结果。
 await page.evaluate(() => { window.__USAGE__.holdRefSearch = true; });
 const refSearchInput = page.getByRole('searchbox', { name: '搜索参考价', exact: true });
 await refSearchInput.fill('gpt');
 await page.waitForFunction(() => window.__USAGE__.calls.filter(c => c.cmd === 'search_reference_prices').length === 1);
 await page.getByRole('button', { name: 'CNY (¥)', exact: true }).click();
 await page.getByLabel('输入单价', { exact: true }).fill('7000');
 await page.getByLabel('输出单价', { exact: true }).fill('14000');
 await page.getByRole('checkbox', { name: '长上下文价格' }).check();
 await page.getByLabel('输入阈值（Token）').fill('200000');
 await page.getByLabel('长上下文输入单价', { exact: true }).fill('14000');
 await page.getByLabel('长上下文输出单价', { exact: true }).fill('28000');
 await page.waitForTimeout(250);
 assert.equal(await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'search_reference_prices').length), 1, 'editing currency/rates must not re-fire reference search');
 await refSearchInput.fill('');
 await page.evaluate(() => { window.__USAGE__.holdRefSearch = false; window.__USAGE__.refSearchResolvers.splice(0).forEach(r => r()); });
 await page.waitForTimeout(50);
 assert.equal(await page.getByRole('group', { name: '参考价匹配结果', exact: true }).count(), 0,
  'late reference search results must be discarded after clearing query');
 const summaryBeforeSave = await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'read_usage_summary').length);
 const pricesBeforeSave = await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'get_model_prices').length);
 await page.getByRole('button', { name: '保存', exact: true }).click();
 await claudeRow.getByText('自定义（CNY）', { exact: true }).waitFor();
 await page.waitForTimeout(100);
 assert.equal(await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'read_usage_summary').length) - summaryBeforeSave, 1, 'custom price save must trigger exactly one summary reload');
 assert.equal(await page.evaluate(() => window.__USAGE__.calls.filter(c => c.cmd === 'get_model_prices').length) - pricesBeforeSave, 1, 'custom price save must trigger exactly one model prices reload');
 assert.ok((await claudeRow.textContent()).includes('¥7,000'));
 assert.ok((await claudeRow.textContent()).includes('$1,000'));
 assert.ok((await claudeRow.textContent()).includes('>200K ¥14,000'));
 await claudeRow.getByRole('button', { name: '编辑', exact: true }).click();
 assert.equal(await page.getByLabel('长上下文输入单价', { exact: true }).inputValue(), '14000');
 await page.getByRole('button', { name: '取消', exact: true }).click();
 // 部分参考价刷新失败保留旧缓存并给出明确反馈
 await page.evaluate(() => { window.__USAGE__.failLitellmRefresh = true; });
 await page.getByRole('button', { name: '更新参考价', exact: true }).click();
 await page.locator('.prices-inline-error').waitFor();
 assert.match(await page.locator('.prices-inline-error').textContent(), /LiteLLM/);
 // 960px 最小宽度下模型价格页无横向溢出
 await page.setViewportSize({ width: 960, height: 900 });
 assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
 await page.setViewportSize({ width: 1280, height: 900 });
 // 切回统计分析，自定义价已立即生效于历史记录
 await page.getByRole('tab', { name: '统计分析', exact: true }).click();
 await page.locator(modelRow('claude-fixture')).getByText('$0.36', { exact: true }).waitFor();
 // 删除覆盖恢复未定价
 await page.getByRole('tab', { name: '模型价格', exact: true }).click();
 await claudeRow.getByRole('button', { name: '删除', exact: true }).click();
 await page.getByRole('dialog').getByRole('button', { name: '删除', exact: true }).click();
 await claudeRow.getByText('未定价', { exact: true }).waitFor();
 assert.equal(await page.evaluate(() => window.__USAGE__.collector.scans.length), syncBeforePrices);

 await page.getByRole('button', { name: '会话', exact: true }).click();
 assert.ok(await page.evaluate(() => window.__USAGE__.calls.some(c => c.cmd === 'cancel_usage_summary')));
 assert.deepEqual(errors, []);
 console.log(`PASS: background retention stays independent; chart labels keep their size and proportions at 960/1280/1600/2200 and after shrinking; pricing sub-tab updates history without rescanning (${engine})`);
} finally { await browser?.close(); rmSync(privateDir, { recursive: true, force: true }); }
