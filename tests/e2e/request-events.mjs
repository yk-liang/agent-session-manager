import { chooseAgent } from '../helpers/agent-switcher.mjs';
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
  fixture.events.find(e => e.status === 'failed').request.project = null;
  browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1080 } });
  const errors = []; page.on('pageerror', e => errors.push(e.message));
  await page.addInitScript(fixture => {
    Object.defineProperty(document, 'hasFocus', { value: () => false });
    const names = { claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', qoder: 'Qoder CLI', pi: 'Pi' };
    const state = window.__EVENTS__ = { fixture, calls: [], fail: false, failCopy: false, delay: false, release: null, revision: '1' };
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
      state.calls.push({ cmd, args });
      if (cmd === 'plugin:clipboard-manager|write_text' && state.failCopy) throw Error('Clipboard unavailable');
      if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, supportsModelNames: false, supportsFastCatalog: false, dataDir: null, deletePolicy: 'trash' }));
      if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (cmd === 'collector_status') return { desired: true, running: true, authorization: 'enabled' };
      if (cmd === 'sync_request_events') return { updatedAt: Date.now(), checkedFiles: 0, changedFiles: 0, bytesRead: 0 };
      if (cmd === 'read_usage_summary') return { rows: fixture.events.map(e => ({ ...e, bucketStart: null, date: new Date(e.time).toISOString().slice(0,10), totalTokens: e.inputTokens + e.outputTokens + e.unclassifiedTokens })), health: {}, syncedAgents: Object.keys(names), revision: '0' };
      if (cmd === 'read_request_events') {
        if (state.fail) throw new Error('fixture read failure');
        const q = args.query;
        const scope = fixture.events.filter(e => (!q.agent || q.agent === e.agent) && (!q.from || e.time >= q.from) && (!q.to || e.time < q.to));
        const filtered = scope.filter(e => (!q.model || q.model === e.model) && (!q.status || q.status === e.status));
        const pageSize = q.limit ?? 50, anchor = q.anchor ? filtered.findIndex(e => e.id === q.anchor) : -1;
        const number = anchor < 0 ? q.page ?? 1 : Math.floor(anchor / pageSize) + 1;
        const page = Math.max(1, Math.min(number, Math.ceil(filtered.length / pageSize)));
        const start = (page - 1) * pageSize;
        const result = { events: filtered.slice(start, start + pageSize), total: filtered.length, page, pageSize, revision: state.revision, models: [...new Set(scope.map(e => e.model))] };
        if (state.delay) { state.delay = false; await new Promise(resolve => state.release = resolve); }
        return structuredClone(result);
      }
      return null;
    } };
  }, fixture);
  await page.goto(baseURL);
  await page.getByRole('button', { name: '用量', exact: true }).click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  const table = page.locator('.request-events');
  assert.equal(await table.locator('tbody > tr').count(), 50);
  const codex = table.locator('tbody > tr').filter({ hasText: 'gpt-codex' });
  assert.ok((await codex.textContent()).includes('≈ 50.0 t/s'));
  assert.ok((await codex.textContent()).includes('本轮首字 400 ms'));
  assert.equal(await codex.locator('.request-source > span').textContent(), 'Codex');
  assert.equal(await codex.locator('.request-source > small').textContent(), 'Codex Desktop');
  assert.equal(await codex.locator('.request-project > span').textContent(), 'codex');
  assert.equal(await codex.locator('.request-project > small').textContent(), '/fixture/codex');
  const beforeExpand = await table.locator('th').nth(5).boundingBox();
  await codex.locator('.request-expand').click();
  assert.equal(await table.locator('.request-detail dl > div').filter({ has: page.getByText('原始来源类型', { exact: true }) }).locator('dd').textContent(), 'vscode');
  const codexEvent = fixture.events.find(event => event.agent === 'codex');
  for (const [label, count, recorded] of [['缓存读取', codexEvent.cacheReadTokens, codexEvent.cacheReadRecorded], ['缓存写入', codexEvent.cacheWriteTokens, codexEvent.cacheWriteRecorded], ['思考 Token', codexEvent.reasoningTokens, codexEvent.reasoningRecorded]]) {
    assert.equal(await table.locator('.request-detail dl > div').filter({ has: page.getByText(label, { exact: true }) }).locator('dd').textContent(), recorded ? count.toLocaleString('en-US') : '未记录');
  }
  const afterExpand = await table.locator('th').nth(5).boundingBox();
  assert.equal(afterExpand.width, beforeExpand.width, 'expanded metadata must not resize status columns');
  await codex.locator('.request-expand').click();
  const project = table.locator('.request-project > small').filter({ hasText: 'example-project' });
  assert.equal(await project.getAttribute('title'), longProject);
  assert.ok((await project.textContent()).startsWith('~/Workspace/'));
  assert.equal(await table.locator('.request-project > span').filter({ hasText: /^\.codex$/ }).count(), 1);
  console.log(`PASS: actual client is preferred, raw source remains in details, project names and full paths remain available (${engine})`);
  const rowIds = () => table.locator('tbody > tr:not(.request-detail-row)').evaluateAll(rows => rows.map(row => row.dataset.eventId));
  const firstIds = await rowIds();
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
  await table.getByLabel('每页请求数', { exact: true }).selectOption('100');
  await page.getByText('第 1–100 条，共 110 条', { exact: true }).waitFor();
  assert.equal((await rowIds()).length, 100);
  await table.getByRole('button', { name: '末页', exact: true }).click();
  await page.getByText('第 101–110 条，共 110 条', { exact: true }).waitFor();
  await table.getByLabel('每页请求数', { exact: true }).selectOption('200');
  await page.getByText('第 1–110 条，共 110 条', { exact: true }).waitFor();
  assert.equal((await rowIds()).length, 110);
  assert.ok(await table.getByRole('button', { name: '下一页', exact: true }).isDisabled());
  await table.getByLabel('每页请求数', { exact: true }).selectOption('50');
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
  await page.getByLabel('请求结果', { exact: true }).selectOption('unknown');
  await page.getByText('共 0 条', { exact: true }).waitFor();
  assert.equal((await rowIds()).length, 0);
  assert.ok(await table.getByRole('button', { name: '下一页', exact: true }).isDisabled());
  assert.equal(await table.locator('.request-page-number').textContent(), '第 1 / 1 页');
  await page.getByLabel('请求结果', { exact: true }).selectOption('failed');
  await page.getByText('第 1–1 条，共 1 条', { exact: true }).waitFor();
  const failed = table.locator('tbody > tr').first();
  assert.equal(await failed.locator('td').nth(7).textContent(), '—');
  assert.equal(await failed.locator('td').nth(10).textContent(), '—');
  assert.equal(await failed.locator('.request-project').textContent(), '未记录');
  await failed.locator('.request-expand').click();
  assert.ok((await table.locator('.request-detail').textContent()).includes('503'));
  assert.ok((await table.locator('.request-detail').textContent()).includes('未记录'));
  await page.evaluate(() => { window.__EVENTS__.failCopy = true; });
  await table.locator('.request-session').click();
  await page.getByText('复制失败，可以展开详情选择会话 ID', { exact: true }).waitFor();
  assert.equal(await table.locator('.request-session').getAttribute('data-copied'), 'false');
  await page.evaluate(() => { window.__EVENTS__.failCopy = false; });
  await table.locator('.request-session').click();
  await table.locator('.request-session[data-copied="true"] .lucide-check').waitFor();
  assert.equal(await page.getByText('复制失败，可以展开详情选择会话 ID', { exact: true }).count(), 0);
  assert.ok(await page.evaluate(() => window.__EVENTS__.calls.some(c => c.cmd.includes('clipboard') && c.args.text === 'session-one')));
  await page.getByLabel('统计模型', { exact: true }).selectOption('claude-sonnet');
  await page.getByText('第 1–1 条，共 1 条', { exact: true }).waitFor();
  await table.getByRole('button', { name: '清除结果筛选', exact: true }).click();
  assert.equal(await page.getByLabel('统计模型', { exact: true }).inputValue(), 'claude-sonnet');
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
    state.revision = '2'; window.dispatchEvent(new Event('blur')); window.dispatchEvent(new Event('focus'));
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
    window.dispatchEvent(new Event('blur')); window.dispatchEvent(new Event('focus'));
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
  const syncCount = await page.evaluate(() => {
    const state = window.__EVENTS__; state.delay = true; state.release = null;
    return state.calls.filter(c => c.cmd === 'sync_request_events').length;
  });
  await table.getByRole('button', { name: '下一页', exact: true }).click();
  await page.waitForFunction(() => !!window.__EVENTS__.release);
  await page.evaluate(() => {
    const state = window.__EVENTS__, latest = state.fixture.events[0];
    state.fixture.events.unshift(...Array.from({ length: 50 }, (_, i) => ({ ...latest, id: `pending-fixture-${i}`, time: latest.time + 1000 * (50 - i) })));
    state.revision = '4'; window.dispatchEvent(new Event('blur')); window.dispatchEvent(new Event('focus'));
  });
  await page.waitForFunction(n => window.__EVENTS__.calls.filter(c => c.cmd === 'sync_request_events').length >= n + 5, syncCount);
  assert.equal(await table.locator('.request-page-number').textContent(), '第 1 / 3 页', 'background refresh must not interrupt navigation');
  await page.evaluate(() => window.__EVENTS__.release());
  await page.getByText('第 101–150 条，共 160 条', { exact: true }).waitFor();
  assert.deepEqual(await rowIds(), secondIds, 'deferred refresh must anchor the newly rendered page');
  await page.evaluate(() => {
    const state = window.__EVENTS__; state.fixture.events.splice(0, 50); state.revision = '5';
    window.dispatchEvent(new Event('blur')); window.dispatchEvent(new Event('focus'));
  });
  await page.getByText('第 51–100 条，共 110 条', { exact: true }).waitFor();
  await table.getByRole('button', { name: '首页', exact: true }).click();
  await page.getByText('第 1–50 条，共 110 条', { exact: true }).waitFor();
  console.log(`PASS: simultaneous pagination and background sync preserve the requested page's records (${engine})`);

  for (const width of [960, 1280, 1600, 2200]) {
    await page.setViewportSize({ width, height: 1080 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    assert.ok(await table.locator('.request-table-wrap').evaluate(el => el.scrollWidth >= el.clientWidth && el.clientWidth > 450));
    assert.ok(await table.locator('.request-footer').evaluate(el => el.scrollWidth <= el.clientWidth), 'pagination controls must fit within the card');
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
