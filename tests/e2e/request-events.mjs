import { addUsageInitScript } from '../helpers/usage-collector.mjs';
import { chooseOption, openDropdown } from '../helpers/dropdown.mjs';
import { chooseAgent } from '../helpers/agent-switcher.mjs';
import { assertTooltipValues } from '../helpers/tooltip.mjs';
import { assertTripleClickSelects } from '../helpers/selection.mjs';
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
  successes[0].request.project = '~/Workspace/agent-session-manager';
  successes[0].request.absoluteProjectPath = longProject;
  successes[1].request.project = '/Users/demo/.codex';
  successes[0].request.latencyMs = 3530;
  successes[0].request.ttftMs = 1670;
  successes[0].request.client = null;
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
    serviceTier: 'flex',
  };
  browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1080 }, deviceScaleFactor: 2 });
  const errors = []; page.on('pageerror', e => errors.push(e.message));
  await addUsageInitScript(page, (fixture, createCollector) => {
    Object.defineProperty(document, 'hasFocus', { value: () => false });
    const names = { claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', qoder: 'Qoder', pi: 'Pi' };
    const state = window.__EVENTS__ = { fixture, calls: [], fail: false, failCopy: false, delay: false, release: null, revision: '1', sessions: {}, delayCatalog: false, releaseCatalog: null };
    state.collector = createCollector([]);
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: state.collector.bridge(async (cmd, args = {}) => {
      state.calls.push({ cmd, args });
      if (cmd === 'plugin:clipboard-manager|write_text' && state.failCopy) throw Error('Clipboard unavailable');
      if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsArchivedSessions: id === 'codex', supportsFork: false, dataDir: null, deletePolicy: 'trash' }));
      if (cmd === 'catalog') {
        if (state.delayCatalog) { state.delayCatalog = false; await new Promise(resolve => state.releaseCatalog = resolve); }
        const sessions = (state.sessions[args.agent] ?? []).filter(session => !session.archived || args.includeArchived);
        return { sessions, projects: sessions.map(session => ({ dirName: session.projectDir, displayPath: session.cwd, sessionCount: 1, lastActivity: null })), warnings: [] };
      }
      if (cmd === 'read_session') {
        const meta = state.sessions[args.agent]?.find(s => s.sessionId === args.sessionId && s.projectDir === args.projectDir);
        if (!meta) throw Error('Unknown session');
        return { meta, events: [{ type: 'user', message: { content: meta.title } }] };
      }
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
            const next = timeline.bounds.findIndex(bound => bound > event.time);
            const i = next < 0 ? buckets.length - 1 : Math.max(0, next - 1);
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
  const usageTab = page.getByRole('button', { name: '用量', exact: true });
  await usageTab.click();
  assert.equal(await usageTab.getAttribute('aria-current'), 'page');
  const requestTab = page.getByRole('tab', { name: '请求记录', exact: true });
  const tabColor = () => requestTab.evaluate(el => getComputedStyle(el).color);
  const resting = await tabColor();
  await requestTab.hover();
  assert.equal(await tabColor(), resting);
  await requestTab.click();
  assert.equal(await requestTab.getAttribute('aria-selected'), 'true');
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  const table = page.locator('.request-events');
  const cacheWriteColumn = table.getByRole('columnheader', { name: '缓存写入', exact: true });
  const tierColumn = table.getByRole('columnheader', { name: '推理 / 档位', exact: true });
  assert.equal(await cacheWriteColumn.count(), 1, 'cache writes on a later page keep the column visible');
  assert.equal(await tierColumn.count(), 1, 'a tier on a later page keeps the combined heading');
  assert.equal(await table.getByRole('columnheader', { name: '客户端', exact: true }).count(), 1);
  assert.equal(await table.locator('tbody > tr').count(), 50);
  const codex = table.locator('tbody > tr').filter({ hasText: 'gpt-codex' });
  assert.equal(await codex.locator('td').nth(7).locator('.usage-num').textContent(), '1,000');
  const amount = codex.locator('.request-cost .usage-num');
  assert.equal(await amount.textContent(), '$0.02');
  assert.equal(await amount.evaluate(el => getComputedStyle(el.parentElement).textDecorationStyle), 'dotted');
  await amount.hover();
  const feeTip = page.getByRole('tooltip');
  await feeTip.getByText('Models.dev 参考价 · flex 档', { exact: true }).waitFor();
  assert.doesNotMatch(await feeTip.textContent(), /含推理/);
  await assertTooltipValues(feeTip);
  await page.keyboard.press('Escape');
  const sessionId = codex.locator('.request-session-id code');
  await sessionId.dblclick();
  assert.ok(await page.evaluate(() => getSelection().toString()), 'session ID text can be selected');
  await page.evaluate(() => getSelection().removeAllRanges());
  await assertTripleClickSelects(codex.locator('.request-project > small'), '/fixture/codex');
  await assertTripleClickSelects(sessionId);
  assert.equal(await codex.locator('.request-timing > span').textContent(), '~50.0 t/s');
  assert.equal(await codex.locator('.request-timing small').textContent(), '400ms · ~2.00s');
  const timing = event => table.locator(`tr[data-event-id=${JSON.stringify(event.id)}] .request-timing small`);
  assert.equal(await timing(successes[0]).textContent(), '1.67s · 3.53s');
  assert.equal(await timing(successes[1]).textContent(), '3.53s');
  assert.equal(await codex.locator('.request-client > span').textContent(), 'Codex Desktop');
  assert.equal(await codex.locator('.request-client > small').textContent(), '0.99.0');
  assert.equal(await codex.locator('.request-project > span').textContent(), 'codex');
  assert.equal(await codex.locator('.request-project > small').textContent(), '/fixture/codex');
  const beforeExpand = await table.locator('th').nth(5).boundingBox();
  await codex.locator('.request-expand').click();
  const detail = table.locator('.request-detail');
  assert.deepEqual(await detail.locator('dt').allTextContents(), ['请求 ID', '会话', '类型', '请求时间', '计时说明', '结束原因', '客户端', '项目']);
  const detailValue = label => detail.locator(':scope > div').filter({ has: page.getByText(label, { exact: true }) }).locator('dd');
  assert.equal(await detailValue('会话').locator('code').textContent(), 'codex-session');
  assert.equal(await detailValue('类型').textContent(), 'vscode');
  assert.match(await detailValue('请求时间').textContent(), /\d{2}:\d{2}:\d{2}\.\d{3} – .+\d{2}:\d{2}:\d{2}\.\d{3}$/);
  assert.equal(await detailValue('计时说明').textContent(), '耗时根据日志估算');
  assert.equal(await detailValue('结束原因').textContent(), 'length');
  assert.equal(await detailValue('客户端').textContent(), await codex.locator('.request-client').getAttribute('title'));
  assert.ok(await detail.locator(':scope > div').evaluateAll(rows => {
    const boxes = rows.map(row => row.getBoundingClientRect());
    const leftEdges = rows.map(row => row.querySelector('dd').getBoundingClientRect().left);
    return boxes.every((box, i) => i === 0 || box.top >= boxes[i - 1].bottom)
      && leftEdges.every(left => Math.abs(left - leftEdges[0]) < 1)
      && rows.every(row => getComputedStyle(row).borderTopWidth === '0px');
  }), 'request details are stacked with a shared value column');
  const afterExpand = await table.locator('th').nth(5).boundingBox();
  assert.equal(afterExpand.width, beforeExpand.width, 'expanded metadata must not resize status columns');
  for (const [label, value] of [['请求 ID', 'response-one'], ['会话 ID', 'codex-session']]) {
    await detail.getByRole('button', { name: `复制${label}` }).click();
    await page.waitForFunction(value => window.__EVENTS__.calls.some(call => call.cmd.includes('clipboard') && call.args.text === value), value);
  }
  await codex.locator('.request-expand').click();
  const worker = table.locator(`tr[data-event-id=${JSON.stringify(successes[0].id)}]`);
  assert.equal(await worker.locator('.request-session-id code').textContent(), 'worker');
  assert.equal(await worker.locator('.request-tier').textContent(), 'standard');
  assert.equal(await worker.locator('.request-client > span').textContent(), 'Claude Code');
  assert.equal(await worker.locator('.request-client > small').textContent(), '2.1.309');
  const cli = table.locator(`tr[data-event-id=${JSON.stringify(successes[1].id)}] .request-client`);
  assert.equal(await cli.locator('span').textContent(), 'Claude Code CLI');
  await worker.locator('.request-expand').click();
  assert.deepEqual((await detail.locator('dt').allTextContents()).slice(0, 4), ['请求 ID', '会话', '类型', '父会话']);
  assert.equal(await detailValue('会话').locator('code').textContent(), 'worker');
  assert.equal(await detailValue('类型').textContent(), 'agent:builtin:web-fetch');
  assert.equal(await detailValue('客户端').textContent(), await worker.locator('.request-client').getAttribute('title'));
  assert.equal(await detailValue('父会话').locator('code').textContent(), 'session-one');
  assert.equal(await detailValue('项目').locator('code').textContent(), longProject);
  await assertTripleClickSelects(detailValue('项目').locator('code'), longProject);
  for (const [label, value] of [['父会话 ID', 'session-one'], ['项目路径', longProject]]) {
    await detail.getByRole('button', { name: `复制${label}` }).click();
    await page.waitForFunction(value => window.__EVENTS__.calls.some(call => call.cmd.includes('clipboard') && call.args.text === value), value);
  }
  await worker.locator('.request-expand').click();
  const project = table.locator('.request-project > small').filter({ hasText: 'example-project' });
  assert.equal(await project.getAttribute('title'), longProject);
  assert.ok((await project.textContent()).startsWith('~/Workspace/'));
  assert.equal(await table.locator('.request-project > span').filter({ hasText: /^\.codex$/ }).count(), 1);
  console.log(`PASS: request details stack without dividers and support copying IDs and absolute project paths (${engine})`);
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
  await page.mouse.move(0, 0);
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
  const freeRequest = table.locator('tbody tr').filter({ hasText: 'demo:free' });
  assert.equal(await freeRequest.locator('.request-tier').textContent(), 'priority');
  const freeCost = freeRequest.locator('.request-cost');
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
  assert.equal(await table.locator('.request-result[data-tone="success"]').count(), 50);
  await chooseOption(page, '请求结果', { label: '失败' });
  await page.getByText('第 1–1 条，共 1 条', { exact: true }).waitFor();
  const failed = table.locator('tbody > tr').first();
  assert.equal(await cacheWriteColumn.count(), 0);
  assert.equal(await failed.locator('td').nth(7).textContent(), '—');
  assert.equal(await failed.locator('td').nth(9).textContent(), '—');
  assert.equal(await failed.locator('.request-project').textContent(), '未记录');
  const failureText = 'HTTP 503 overloaded\nThe provider is temporarily unavailable.\nTry again in a moment.';
  await failed.locator('.request-result[data-tone="error"]').hover();
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
  await assertTripleClickSelects(failureDetail.locator('pre'), 'The provider is temporarily unavailable.');
  await failureDetail.getByRole('button', { name: '复制错误信息' }).click();
  await failureDetail.locator('[data-copied="true"]').waitFor();
  assert.ok(await page.evaluate(value => window.__EVENTS__.calls.some(c => c.cmd.includes('clipboard') && c.args.text === value), failureText));
  assert.doesNotMatch(await table.locator('.request-detail').textContent(), /费用明细|日志文件/);
  assert.equal(await failed.locator('.request-cost .usage-num').textContent(), '$0.00');
  await failed.locator('.request-cost .tip-trigger').hover();
  await page.getByRole('tooltip').getByText('失败请求未记录 Token 用量', { exact: true }).waitFor();
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
    state.delay = true;
    state.release = null;
    state.fixture.events.unshift(...Array.from({ length: 50 }, (_, i) => ({ ...latest, id: `new-fixture-${i}`, time: latest.time + 1000 * (50 - i) })));
    state.revision = '2'; state.collector.changed();
  });
  await page.waitForFunction(() => !!window.__EVENTS__.release);
  await scroller.evaluate(el => { el.scrollTop += 10; });
  const afterScroll = await readingOffset();
  await page.evaluate(() => { window.__EVENTS__.release(); window.__EVENTS__.release = null; });
  await page.getByText('第 101–150 条，共 160 条', { exact: true }).waitFor();
  assert.equal(await table.locator(`tr[data-event-id="${readerId}"] .request-expand`).getAttribute('aria-expanded'), 'true');
  const afterSync = await readingOffset();
  assert.ok(Math.abs(afterSync.offset - afterScroll.offset) <= 1, 'a dropped refresh retries at the new scroll position without another revision');
  assert.equal(afterSync.left, beforeSync.left);
  assert.equal((await rowIds()).length, 50);
  assert.ok(await page.evaluate(id => window.__EVENTS__.calls.some(c => c.cmd === 'read_request_events' && c.args.query.anchor === id), readerId));
  await page.evaluate(() => {
    const state = window.__EVENTS__;
    state.fixture.events.splice(0, 50); state.revision = '3';
    state.collector.changed();
  });
  await page.getByText('第 51–100 条，共 110 条', { exact: true }).waitFor();
  assert.ok(Math.abs((await readingOffset()).offset - afterScroll.offset) <= 1);
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

  const noon = new Date(2026, 10, 23, 12);
  await page.clock.setFixedTime(noon);
  await page.evaluate(time => {
    const state = window.__EVENTS__;
    const start = time - 12 * 3600000;
    state.fixture.timeline = { bounds: Array.from({ length: 25 }, (_, i) => start + i * 3600000), success: Array(24).fill(0), failed: Array(24).fill(0), untimed: { success: 0, failed: 0 } };
    state.fixture.events.forEach((event, i) => { event.time = time - 60000 * i; });
    const meta = (sessionId, projectDir, cwd, title, archived = false) => ({ sessionId, projectDir, cwd, title, archived,
      hasTranscript: true, userCount: 1, realUserCount: 1, assistantCount: 0, assistantTextCount: 0, sizeBytes: 100 });
    state.sessions.codex = [meta('codex-session', 'codex-source', '/fixture/codex-start', '关联的归档 Codex 会话', true)];
    const workerRequest = state.fixture.events.find(event => event.request.subagentId === 'worker');
    state.sessions.claude = [
      meta('worker', 'wrong-a', '/fixture/unrelated-a', '同 ID 的子会话 A'),
      meta('worker', 'wrong-b', '/fixture/unrelated-b', '同 ID 的子会话 B'),
      meta('session-one', 'claude-source', workerRequest.request.absoluteProjectPath, '关联的 Claude 主会话'),
    ];
  }, noon.getTime());
  await usageTab.click();
  await page.getByRole('button', { name: '今日', exact: true }).click();
  const axis = table.locator('.request-timeline-axis');
  const nowMarker = axis.getByText('现在', { exact: true });
  await nowMarker.waitFor();
  for (const width of [960, 1600]) {
    await page.setViewportSize({ width, height: 1080 });
    assert.equal(await axis.getByText('12:00', { exact: true }).isVisible(), false);
    await axis.getByText('06:00', { exact: true }).waitFor();
    assert.ok(await axis.locator('span').evaluateAll(labels => {
      const boxes = labels.filter(label => getComputedStyle(label).visibility === 'visible')
        .map(label => label.getBoundingClientRect()).sort((a, b) => a.left - b.left);
      return boxes.length > 2 && boxes.every((box, i) => i === 0 || box.left >= boxes[i - 1].right);
    }));
  }
  console.log(`PASS: the noon marker keeps nearby ticks from overlapping at narrow and wide widths (${engine})`);

  const openCodex = codex.getByRole('button', { name: '打开关联会话 codex-session', exact: true });
  await page.evaluate(() => { window.__EVENTS__.delayCatalog = true; });
  await openCodex.click();
  await page.waitForFunction(() => !!window.__EVENTS__.releaseCatalog);
  await page.getByRole('tab', { name: '统计分析', exact: true }).click();
  await page.evaluate(() => { window.__EVENTS__.releaseCatalog(); });
  await requestTab.click();
  assert.equal(await usageTab.getAttribute('aria-current'), 'page');

  await page.evaluate(() => { window.__EVENTS__.fail = true; });
  await table.getByRole('button', { name: '下一页', exact: true }).click();
  const retryPage = table.getByRole('button', { name: '重试', exact: true });
  await retryPage.waitFor();
  await page.evaluate(() => { window.__EVENTS__.fail = false; });
  await page.evaluate(() => { const state = window.__EVENTS__; state.delayCatalog = true; state.releaseCatalog = null; });
  await openCodex.click();
  await page.waitForFunction(() => !!window.__EVENTS__.releaseCatalog);
  await retryPage.click();
  await page.evaluate(() => { window.__EVENTS__.releaseCatalog(); });
  await page.getByText('第 51–100 条，共 110 条', { exact: true }).waitFor();
  assert.equal(await usageTab.getAttribute('aria-current'), 'page');
  await table.getByRole('button', { name: '首页', exact: true }).click();

  await openCodex.click();
  await page.locator('.detail-body').getByText('关联的归档 Codex 会话', { exact: true }).waitFor();
  assert.ok(await page.evaluate(() => window.__EVENTS__.calls.some(call => call.cmd === 'read_session' && call.args.agent === 'codex' && call.args.projectDir === 'codex-source')));
  assert.equal(await page.getByRole('checkbox', { name: '显示已归档', exact: true }).isChecked(), true);
  await page.getByRole('checkbox', { name: '显示空会话', exact: true }).check();
  assert.ok(await page.locator('.detail-body').getByText('关联的归档 Codex 会话', { exact: true }).isVisible());

  await usageTab.click();
  const openWorker = worker.getByRole('button', { name: '打开关联会话 worker', exact: true });
  await page.evaluate(() => {
    const state = window.__EVENTS__;
    state.parent = state.sessions.claude.pop();
    state.sessions.claude.push({ ...state.parent, cwd: '/fixture/unrelated-parent' });
  });
  await openWorker.click();
  await page.getByText('未找到可打开的关联会话。', { exact: true }).waitFor();
  await page.evaluate(() => {
    const state = window.__EVENTS__;
    state.sessions.claude.pop();
    state.sessions.claude.push(state.parent);
  });
  await openWorker.click();
  await page.locator('.detail-body').getByText('关联的 Claude 主会话', { exact: true }).waitFor();
  assert.ok(await page.evaluate(() => window.__EVENTS__.calls.some(call => call.cmd === 'read_session' && call.args.agent === 'claude' && call.args.sessionId === 'session-one')));
  assert.deepEqual(errors, []);
  console.log(`PASS: request rows open the correct cross-agent session, resolve the parent and cancel outdated navigation (${engine})`);
} finally { await browser?.close(); rmSync(dir, { recursive: true, force: true }); }
