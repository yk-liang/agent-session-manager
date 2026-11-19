import { addUsageInitScript } from '../helpers/usage-collector.mjs';
import { chooseOption, openDropdown } from '../helpers/dropdown.mjs';
import { chooseAgent } from '../helpers/agent-switcher.mjs';
import { assertTooltipValues } from '../helpers/tooltip.mjs';
import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const dir = mkdtempSync(path.join(tmpdir(), 'asm-events-browser-'));
let browser;
try {
  const exported = path.join(dir, 'events.json');
  const run = await promisify(execFile)('cargo', ['test', '--offline', '--locked', 'export_request_events_fixture_if_requested', '--', '--nocapture'], {
    cwd: path.resolve('src-tauri'), env: { ...process.env, ASM_REQUEST_EVENTS_EXPORT: exported }, timeout: 180000,
  });
  assert.match(run.stdout, /1 passed/);
  const fixture = JSON.parse(readFileSync(exported, 'utf8'));
  assert.equal(fixture.total, 110);
  const longProject = `/Users/demo/Workspace/${'nested/'.repeat(24)}example-project`;
  const successes = fixture.events.filter(e => e.agent === 'claude' && e.status === 'success');
  successes[0].request.project = longProject;
  successes[1].request.project = '/Users/demo/.codex';
  successes[0].request.latencyMs = 3530;
  successes[0].request.ttftMs = 1670;
  successes[1].request.latencyMs = 3530;
  successes[1].request.ttftMs = null;
  for (const event of fixture.events.slice(0, -1)) event.cacheWriteTokens = 0;
  assert.ok(fixture.events.at(-1).cacheWriteTokens > 0);
  assert.equal(fixture.events.at(-1).request.tier, 'priority');
  fixture.events.find(e => e.status === 'failed').request.project = null;
  const codexEvent = fixture.events.find(e => e.agent === 'codex');
  codexEvent.request.tier = null;
  codexEvent.request.stop = 'length';
  codexEvent.cost = {
    source: 'models.dev', totalUsd: 0.02,
    inputRate: 20, inputUsd: 0.02, outputRate: 0, outputUsd: 0,
  };
  browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1080 } });
  const errors = []; page.on('pageerror', e => errors.push(e.message));
  await addUsageInitScript(page, (fixture, createCollector) => {
    Object.defineProperty(document, 'hasFocus', { value: () => false });
    const names = { claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', qoder: 'Qoder', pi: 'Pi' };
    const state = window.__EVENTS__ = { fixture, calls: [], fail: false, failCopy: false, delay: false, release: null, revision: '1' };
    state.collector = createCollector([]);
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: state.collector.bridge(async (cmd, args = {}) => {
      state.calls.push({ cmd, args });
      if (cmd === 'plugin:clipboard-manager|write_text' && state.failCopy) throw Error('Clipboard unavailable');
      if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, dataDir: null, deletePolicy: 'trash' }));
      if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (cmd === 'read_usage_summary') return { rows: fixture.events.map(e => ({ ...e, bucketStart: null, date: new Date(e.time).toISOString().slice(0,10), totalTokens: e.inputTokens + e.outputTokens + e.unclassifiedTokens })), syncedAgents: Object.keys(names), revision: '0' };
      if (cmd === 'read_request_events') {
        if (state.fail) throw new Error('fixture read failure');
        const q = args.query;
        const scope = fixture.events.filter(e => (!q.agent || q.agent === e.agent) && (!q.from || e.time >= q.from) && (!q.to || e.time < q.to));
        const match = (event, facet) =>
          (facet === 'models' || q.model == null || event.model === q.model)
          && (facet === 'projects' || q.project == null || (event.request.project ?? '') === q.project)
          && (facet === 'results' || q.status == null || event.status === q.status);
        const filtered = scope.filter(e => match(e));
        const filters = Object.fromEntries(Object.entries({
          models: e => e.model, projects: e => e.request.project ?? '', results: e => e.status,
        }).map(([facet, key]) => {
          const counts = {};
          for (const event of scope.filter(e => match(e, facet))) counts[key(event)] = (counts[key(event)] ?? 0) + 1;
          return [facet, counts];
        }));
        const pageSize = q.limit ?? 50, anchor = q.anchor ? filtered.findIndex(e => e.id === q.anchor) : -1;
        const number = anchor < 0 ? q.page ?? 1 : Math.floor(anchor / pageSize) + 1;
        const page = Math.max(1, Math.min(number, Math.ceil(filtered.length / pageSize)));
        const start = (page - 1) * pageSize;
        const timeline = fixture.timeline && {
          ...fixture.timeline,
          success: fixture.timeline.success.map(() => 0),
          failed: fixture.timeline.failed.map(() => 0),
        };
        if (timeline) {
          for (const event of filtered) {
            const buckets = timeline[event.status];
            const i = Math.max(0, Math.min(buckets.length - 1, Math.floor((event.time - timeline.start) / timeline.step)));
            buckets[i]++;
          }
        }
        const result = { events: filtered.slice(start, start + pageSize), total: filtered.length,
          hasCacheWrites: filtered.some(event => event.cacheWriteTokens > 0),
          hasServiceTiers: filtered.some(event => event.request.tier?.trim()),
          page, pageSize, revision: state.revision, timeline, filters };
        if (state.delay) { state.delay = false; await new Promise(resolve => state.release = resolve); }
        return structuredClone(result);
      }
      return null;
    }) };
  }, fixture);
  await page.goto(baseURL);
  await page.getByRole('button', { name: '用量', exact: true }).click();
  const requestTab = page.getByRole('tab', { name: '请求记录', exact: true });
  const tabColor = () => requestTab.evaluate(el => getComputedStyle(el).color);
  const resting = await tabColor();
  await requestTab.hover();
  assert.equal(await tabColor(), resting);
  await requestTab.click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  const table = page.locator('.request-events');
  const cacheWriteColumn = table.getByRole('columnheader', { name: '缓存写入', exact: true });
  const tierColumn = table.getByRole('columnheader', { name: '推理 / 档位', exact: true });
  assert.equal(await cacheWriteColumn.count(), 1, 'cache writes on a later page keep the column visible');
  assert.equal(await tierColumn.count(), 1, 'a tier on a later page keeps the combined heading');
  assert.equal(await table.locator('tbody > tr').count(), 50);
  const codex = table.locator('tbody > tr').filter({ hasText: 'gpt-codex' });
  assert.equal(await codex.locator('td').nth(7).locator('.usage-num').textContent(), '1,000');
  const amount = codex.locator('.request-cost .usage-num');
  assert.equal(await amount.textContent(), '$0.02');
  assert.equal(await amount.evaluate(el => getComputedStyle(el.parentElement).textDecorationStyle), 'dotted');
  await amount.hover();
  const feeTip = page.getByRole('tooltip');
  await feeTip.getByText('Models.dev 参考价', { exact: true }).waitFor();
  assert.doesNotMatch(await feeTip.textContent(), /含推理/);
  await assertTooltipValues(feeTip);
  await page.keyboard.press('Escape');
  const sessionId = codex.locator('.request-session-id code');
  await sessionId.dblclick();
  assert.ok(await page.evaluate(() => getSelection().toString()), 'session ID text can be selected');
  await page.evaluate(() => getSelection().removeAllRanges());
  assert.ok((await codex.textContent()).includes('≈ 50.0 t/s'));
  assert.equal(await codex.locator('.request-timing small').textContent(), '400ms · ≈ 2.00s');
  const timing = event => table.locator(`tr[data-event-id=${JSON.stringify(event.id)}] .request-timing small`);
  assert.equal(await timing(successes[0]).textContent(), '1.67s · 3.53s');
  assert.equal(await timing(successes[1]).textContent(), '3.53s');
  assert.equal(await codex.locator('.request-source > span').textContent(), 'Codex');
  assert.equal(await codex.locator('.request-source > small').textContent(), 'Codex Desktop');
  assert.equal(await codex.locator('.request-project > span').textContent(), 'codex');
  assert.equal(await codex.locator('.request-project > small').textContent(), '/fixture/codex');
  const beforeExpand = await table.locator('th').nth(5).boundingBox();
  await codex.locator('.request-expand').click();
  const detail = table.locator('.request-detail');
  assert.deepEqual(await detail.locator('dt').allTextContents(), ['请求 ID', '请求时间', '计时说明', '结束原因', '客户端', '项目']);
  const detailValue = label => detail.locator('dl > div').filter({ has: page.getByText(label, { exact: true }) }).locator('dd');
  assert.match(await detailValue('请求时间').textContent(), /\d{2}:\d{2}:\d{2}\.\d{3} – .+\d{2}:\d{2}:\d{2}\.\d{3}$/);
  assert.equal(await detailValue('计时说明').textContent(), '耗时根据日志估算');
  assert.equal(await detailValue('结束原因').textContent(), '达到输出上限');
  assert.equal(await detailValue('客户端').textContent(), 'Codex Desktop 0.99.0');
  assert.ok(await detail.locator('dl > div').evaluateAll(rows => {
    const boxes = rows.map(row => row.getBoundingClientRect());
    const leftEdges = rows.map(row => row.querySelector('dd').getBoundingClientRect().left);
    return boxes.every((box, i) => i === 0 || box.top >= boxes[i - 1].bottom)
      && leftEdges.every(left => Math.abs(left - leftEdges[0]) < 1);
  }), 'request details are stacked with a shared value column');
  const afterExpand = await table.locator('th').nth(5).boundingBox();
  assert.equal(afterExpand.width, beforeExpand.width, 'expanded metadata must not resize status columns');
  for (const [label, value] of [['请求 ID', 'response-one'], ['项目路径', '/fixture/codex']]) {
    await detail.getByRole('button', { name: `复制${label}` }).click();
    await page.waitForFunction(value => window.__EVENTS__.calls.some(call => call.cmd.includes('clipboard') && call.args.text === value), value);
  }
  await codex.locator('.request-expand').click();
  const project = table.locator('.request-project > small').filter({ hasText: 'example-project' });
  assert.equal(await project.getAttribute('title'), longProject);
  assert.ok((await project.textContent()).startsWith('~/Workspace/'));
  assert.equal(await table.locator('.request-project > span').filter({ hasText: /^\.codex$/ }).count(), 1);
  console.log(`PASS: request details stack in order and support copying the request ID and project path (${engine})`);
  const rowIds = () => table.locator('tbody > tr:not(.request-detail-row)').evaluateAll(rows => rows.map(row => row.dataset.eventId));
  const firstIds = await rowIds();
  await chooseOption(page, '项目', { label: '/fixture/codex' });
  await page.getByText('第 1–1 条，共 1 条', { exact: true }).waitFor();
  assert.ok((await table.locator('.request-model').textContent()).includes('gpt-codex'));
  assert.equal(await cacheWriteColumn.count(), 0);
  await table.getByRole('columnheader', { name: '推理', exact: true }).waitFor();
  assert.equal(await tierColumn.count(), 0);
  await table.getByRole('button', { name: '清除筛选', exact: true }).click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  const timeline = table.getByRole('slider', { name: /请求时间分布/ });
  const populatedBuckets = timeline.locator('.request-timeline-bucket:has(i)');
  const tip = page.getByRole('tooltip');
  for (const bucket of [populatedBuckets.last(), populatedBuckets.first()]) {
    await bucket.hover();
    await assertTooltipValues(tip);
    assert.match(await tip.locator('dd').last().textContent(), /^[\d,]+$/);
  }
  await timeline.press('Home');
  await timeline.press('ArrowRight');
  await assertTooltipValues(tip);
  await timeline.press('Home');
  await timeline.press('Enter');
  await page.getByText('第 101–110 条，共 110 条', { exact: true }).waitFor();
  assert.equal(await cacheWriteColumn.count(), 1);
  const freeCost = table.locator('tbody tr').filter({ hasText: 'demo:free' }).locator('.request-cost');
  assert.equal(await freeCost.locator('.usage-num').textContent(), '$0.00');
  assert.equal(await freeCost.locator('small').textContent(), '内置');
  await table.getByRole('button', { name: '首页', exact: true }).click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  assert.ok(await table.getByRole('button', { name: '上一页', exact: true }).isDisabled());
  await table.getByRole('button', { name: '下一页', exact: true }).click();
  await page.getByText('第 51–100 条，共 110 条', { exact: true }).waitFor();
  const secondIds = await rowIds();
  assert.equal(secondIds.length, 50, 'pagination replaces rows instead of appending');
  await table.getByRole('button', { name: '下一页', exact: true }).click();
  await page.getByText('第 101–110 条，共 110 条', { exact: true }).waitFor();
  assert.ok(await table.getByRole('button', { name: '下一页', exact: true }).isDisabled());
  assert.equal(new Set([...firstIds, ...secondIds, ...await rowIds()]).size, 110);
  await table.getByRole('button', { name: '上一页', exact: true }).click();
  await page.getByText('第 51–100 条，共 110 条', { exact: true }).waitFor();
  assert.deepEqual(await rowIds(), secondIds);
  await table.getByRole('button', { name: '首页', exact: true }).click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  await table.getByLabel('请求页码', { exact: true }).fill('3');
  await table.getByLabel('请求页码', { exact: true }).press('Enter');
  await page.getByText('第 101–110 条，共 110 条', { exact: true }).waitFor();
  await chooseOption(page, '每页请求数', '100');
  await page.getByText('第 1–100 条，共 110 条', { exact: true }).waitFor();
  assert.equal((await rowIds()).length, 100);
  await table.getByRole('button', { name: '末页', exact: true }).click();
  await page.getByText('第 101–110 条，共 110 条', { exact: true }).waitFor();
  await chooseOption(page, '每页请求数', '200');
  await page.getByText('第 1–110 条，共 110 条', { exact: true }).waitFor();
  assert.equal((await rowIds()).length, 110);
  assert.ok(await table.getByRole('button', { name: '下一页', exact: true }).isDisabled());
  await chooseOption(page, '每页请求数', '50');
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  await table.getByLabel('请求页码', { exact: true }).fill('999');
  await table.getByRole('button', { name: '跳转', exact: true }).click();
  await page.getByText('第 101–110 条，共 110 条', { exact: true }).waitFor();
  await table.getByLabel('请求页码', { exact: true }).fill('0');
  await table.getByRole('button', { name: '跳转', exact: true }).click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  assert.equal(await table.getByLabel('请求页码', { exact: true }).inputValue(), '1');
  console.log(`PASS: numbered pages replace rows without duplication; navigation, jumps and 50/100/200 page sizes (${engine})`);

  const selectAgent = name => chooseAgent(page, { label: name });
  await selectAgent('Claude Code');
  await page.getByText('第 1–50 条，共 109 条', { exact: true }).waitFor();
  const results = await openDropdown(page, '请求结果');
  const options = results.getByRole('option');
  assert.equal(await options.count(), 3);
  for (const name of ['全部结果', '失败', '成功']) {
    assert.equal(await results.getByRole('option', { name, exact: true }).count(), 1);
  }
  await results.getByRole('option', { name: '成功', exact: true }).click();
  await page.getByText('第 1–50 条，共 108 条', { exact: true }).waitFor();
  assert.equal(await table.locator('.request-result.success').count(), 50);
  await chooseOption(page, '请求结果', { label: '失败' });
  await page.getByText('第 1–1 条，共 1 条', { exact: true }).waitFor();
  const failed = table.locator('tbody > tr').first();
  assert.equal(await cacheWriteColumn.count(), 0);
  assert.equal(await failed.locator('td').nth(7).textContent(), '—');
  assert.equal(await failed.locator('td').nth(9).textContent(), '—');
  assert.equal(await failed.locator('.request-project').textContent(), '未记录');
  const failureText = 'HTTP 503 overloaded\nThe provider is temporarily unavailable.\nTry again in a moment.';
  await failed.locator('.request-result.failed').hover();
  const failureTip = page.getByRole('tooltip');
  await failureTip.getByText('错误信息', { exact: true }).waitFor();
  assert.equal(await failureTip.locator('pre').textContent(), failureText);
  await page.keyboard.press('Escape');
  await failureTip.waitFor({ state: 'hidden' });
  await failed.locator('.request-expand').click();
  assert.equal(
    await table.locator('.request-detail-row > td').getAttribute('colspan'),
    String(await failed.locator('td').count()),
  );
  const failureDetail = table.getByRole('region', { name: '错误信息' });
  assert.equal(await detail.locator('dt').first().textContent(), '错误信息');
  assert.equal(await failureDetail.locator('pre').textContent(), failureText);
  await failureDetail.getByRole('button', { name: '复制错误信息' }).click();
  await failureDetail.locator('[data-copied="true"]').waitFor();
  assert.ok(await page.evaluate(value => window.__EVENTS__.calls.some(c => c.cmd.includes('clipboard') && c.args.text === value), failureText));
  assert.doesNotMatch(await table.locator('.request-detail').textContent(), /费用明细|日志文件/);
  await failed.locator('.request-cost .tip-trigger').hover();
  await page.getByRole('tooltip').getByText(/缺少完整的输入或输出 Token/).waitFor();
  await page.keyboard.press('Escape');
  await page.evaluate(() => { window.__EVENTS__.failCopy = true; });
  await table.locator('.request-session').click();
  await page.getByText('复制失败，请重试。', { exact: true }).waitFor();
  assert.equal(await table.locator('.request-session').getAttribute('data-copied'), 'false');
  await page.evaluate(() => { window.__EVENTS__.failCopy = false; });
  await table.locator('.request-session').click();
  await table.locator('.request-session[data-copied="true"] .lucide-check').waitFor();
  assert.equal(await page.getByText('复制失败，请重试。', { exact: true }).count(), 0);
  assert.ok(await page.evaluate(() => window.__EVENTS__.calls.some(c => c.cmd.includes('clipboard') && c.args.text === 'session-one')));
  await chooseOption(page, '模型', { label: 'claude-sonnet' });
  await page.getByText('第 1–1 条，共 1 条', { exact: true }).waitFor();
  await table.getByRole('button', { name: '清除筛选', exact: true }).click();
  assert.equal(await page.getByLabel('模型', { exact: true }).getAttribute('value'), 'all');
  assert.equal(await page.getByRole('combobox', { name: '当前 Agent', exact: true }).getAttribute("value"), 'claude');
  await selectAgent('全部 Agent');
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  assert.equal(await page.getByRole('combobox', { name: '当前 Agent', exact: true }).getAttribute("value"), 'all');
  console.log(`PASS: shared Agent/model filters, results, details, unknown vs zero, and copying full session IDs (${engine})`);

  await page.evaluate(() => { window.__EVENTS__.delay = true; });
  await table.getByRole('button', { name: '下一页', exact: true }).click();
  await page.waitForFunction(() => !!window.__EVENTS__.release);
  await selectAgent('Codex');
  await page.getByText('第 1–1 条，共 1 条', { exact: true }).waitFor();
  await page.evaluate(() => window.__EVENTS__.release());
  await page.waitForTimeout(50);
  assert.equal(await table.locator('tbody > tr').count(), 1);
  await page.evaluate(() => { window.__EVENTS__.fail = true; });
  await selectAgent('全部 Agent');
  await table.getByRole('alert').waitFor();
  await page.evaluate(() => { window.__EVENTS__.fail = false; });
  await table.getByRole('button', { name: '重试', exact: true }).click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  await page.evaluate(() => { window.__EVENTS__.fail = true; });
  await table.getByRole('button', { name: '下一页', exact: true }).click();
  await table.getByRole('alert').waitFor();
  assert.deepEqual(await rowIds(), firstIds, 'failed navigation keeps the existing page');
  await page.evaluate(() => { window.__EVENTS__.fail = false; });
  await table.getByRole('button', { name: '重试', exact: true }).click();
  await page.getByText('第 51–100 条，共 110 条', { exact: true }).waitFor();
  console.log(`PASS: changing filters rejects late pages; failed navigation retains data and retries the requested page (${engine})`);

  // Insert a full page while reading an older request. Its page number changes,
  // but its expanded detail and visual position should survive the background sync.
  const readerRow = table.locator('tbody > tr:not(.request-detail-row)').nth(8);
  const readerId = await readerRow.getAttribute('data-event-id');
  await readerRow.locator('.request-expand').click();
  const scroller = table.locator('.request-table-wrap');
  await scroller.evaluate((el, id) => {
    const row = [...el.querySelectorAll('tbody > tr')].find(row => row.dataset.eventId === id);
    el.scrollTop += row.getBoundingClientRect().top - el.getBoundingClientRect().top - el.querySelector('thead').getBoundingClientRect().height + 4;
    el.scrollLeft = 100;
  }, readerId);
  const readingOffset = () => scroller.evaluate((el, id) => {
    const row = [...el.querySelectorAll('tbody > tr')].find(row => row.dataset.eventId === id);
    return { offset: row.getBoundingClientRect().top - el.getBoundingClientRect().top, left: el.scrollLeft };
  }, readerId);
  const beforeSync = await readingOffset();
  await page.evaluate(() => {
    const state = window.__EVENTS__, latest = state.fixture.events[0];
    state.fixture.events.unshift(...Array.from({ length: 50 }, (_, i) => ({ ...latest, id: `new-fixture-${i}`, time: latest.time + 1000 * (50 - i) })));
    state.revision = '2'; state.collector.changed();
  });
  await page.getByText('第 101–150 条，共 160 条', { exact: true }).waitFor();
  assert.equal(await table.locator(`tr[data-event-id="${readerId}"] .request-expand`).getAttribute('aria-expanded'), 'true');
  const afterSync = await readingOffset();
  assert.ok(Math.abs(afterSync.offset - beforeSync.offset) <= 1, 'background sync preserves the visible request position');
  assert.equal(afterSync.left, beforeSync.left);
  assert.equal((await rowIds()).length, 50);
  assert.ok(await page.evaluate(id => window.__EVENTS__.calls.some(c => c.cmd === 'read_request_events' && c.args.query.anchor === id), readerId));
  await page.evaluate(() => {
    const state = window.__EVENTS__;
    state.fixture.events.splice(0, 50); state.revision = '3';
    state.collector.changed();
  });
  await page.getByText('第 51–100 条，共 110 条', { exact: true }).waitFor();
  assert.ok(Math.abs((await readingOffset()).offset - beforeSync.offset) <= 1);
  await table.getByRole('button', { name: '首页', exact: true }).click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  assert.equal(await table.locator('.request-detail-row').count(), 0);
  assert.equal(await scroller.evaluate(el => el.scrollTop), 0);
  console.log(`PASS: background insertions/removals keep the visible request and expanded details; manual navigation resets only table scrolling (${engine})`);

  // A sync completing while a page query is pending must wait for that page to
  // render before capturing an anchor; otherwise it anchors the page we just left.
  await page.evaluate(() => {
    const state = window.__EVENTS__; state.delay = true; state.release = null;
  });
  await table.getByRole('button', { name: '下一页', exact: true }).click();
  await page.waitForFunction(() => !!window.__EVENTS__.release);
  await page.evaluate(() => {
    const state = window.__EVENTS__, latest = state.fixture.events[0];
    state.fixture.events.unshift(...Array.from({ length: 50 }, (_, i) => ({ ...latest, id: `pending-fixture-${i}`, time: latest.time + 1000 * (50 - i) })));
    state.revision = '4'; state.collector.changed();
  });
  assert.equal(await table.locator('.request-page-number').textContent(), '第 1 / 3 页', 'background refresh must not interrupt navigation');
  await page.evaluate(() => window.__EVENTS__.release());
  await page.getByText('第 101–150 条，共 160 条', { exact: true }).waitFor();
  assert.deepEqual(await rowIds(), secondIds, 'deferred refresh must anchor the newly rendered page');
  await page.evaluate(() => {
    const state = window.__EVENTS__; state.fixture.events.splice(0, 50); state.revision = '5';
    state.collector.changed();
  });
  await page.getByText('第 51–100 条，共 110 条', { exact: true }).waitFor();
  await table.getByRole('button', { name: '首页', exact: true }).click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  console.log(`PASS: simultaneous pagination and background sync preserve the requested page's records (${engine})`);

  for (const width of [960, 1280, 1600, 2200]) {
    await page.setViewportSize({ width, height: 1080 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    assert.ok(await table.locator('.request-table-wrap').evaluate(el => el.scrollWidth >= el.clientWidth && el.clientWidth > 450));
    assert.ok(await table.locator('.request-footer').evaluate(el => el.scrollWidth <= el.clientWidth), `request footer must fit within the card at ${width}px`);
    const layout = await table.locator('tbody > tr').first().evaluate(row => {
      const result = row.querySelector('.request-result'), timing = row.querySelector('.request-timing');
      return { statusWidth: result.parentElement.getBoundingClientRect().width,
        gap: timing.querySelector('span').getBoundingClientRect().left - result.getBoundingClientRect().right };
    });
    assert.ok(layout.statusWidth <= 80 && layout.gap >= 0 && layout.gap <= 48, `unexpected result/TPS gap at ${width}: ${JSON.stringify(layout)}`);
    assert.ok(await project.evaluate(el => el.scrollWidth > el.clientWidth), 'long paths should truncate inside their column');
  }
  await page.setViewportSize({ width: 1600, height: 1100 });
  await table.scrollIntoViewIfNeeded();
  await table.locator('.request-table-wrap').evaluate(el => { el.scrollLeft = 0; el.scrollTop = 0; });
  await page.mouse.move(0, 0);
  await table.screenshot({ path: path.join(artifactDir, 'request-events.png') });
  await table.locator('.request-table-wrap').evaluate(el => { el.scrollLeft = el.scrollWidth; });
  await table.screenshot({ path: path.join(artifactDir, 'request-events-projects.png') });
  assert.deepEqual(errors, []);
  await page.getByRole('button', { name: '会话', exact: true }).click();
  assert.ok(await page.evaluate(() => window.__EVENTS__.calls.some(c => c.cmd === 'cancel_request_events')));
  console.log(`PASS: bounded 960/1280/1600/2200 layouts, screenshot, cancellation, no runtime errors (${engine})`);
} finally { await browser?.close(); rmSync(dir, { recursive: true, force: true }); }
