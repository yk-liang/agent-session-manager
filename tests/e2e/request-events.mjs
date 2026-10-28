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
    const names = { claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', qoder: 'Qoder CLI', pi: 'Pi' };
    const state = window.__EVENTS__ = { fixture, calls: [], fail: false, failCopy: false, delay: false, release: null, revision: '1' };
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
      state.calls.push({ cmd, args });
      if (cmd === 'plugin:clipboard-manager|write_text' && state.failCopy) throw Error('Clipboard unavailable');
      if (cmd === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: true, supportsFork: false, supportsModelNames: false, supportsFastCatalog: false, dataDir: null, deletePolicy: 'trash' }));
      if (cmd === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (cmd === 'collector_status') return { desired: true, running: true, authorization: 'enabled' };
      if (cmd === 'sync_request_events') return { updatedAt: Date.now(), checkedFiles: 0, changedFiles: 0, bytesRead: 0 };
      if (cmd === 'read_usage_summary') return { rows: [], health: {}, revision: '0' };
      if (cmd === 'read_request_events') {
        if (state.fail) throw new Error('fixture read failure');
        const q = args.query;
        const scope = fixture.events.filter(e => (!q.agent || q.agent === e.agent) && (!q.from || e.time >= q.from) && (!q.to || e.time < q.to));
        const filtered = scope.filter(e => (!q.model || q.model === e.model) && (!q.status || q.status === e.status));
        const reset = !!q.cursor && q.cursor.revision !== state.revision;
        const start = q.cursor && !reset ? filtered.findIndex(e => e.id === q.cursor.id) + 1 : 0;
        const events = filtered.slice(start, start + 50), last = events.at(-1);
        const result = { events, total: filtered.length, reset, revision: state.revision, models: [...new Set(scope.map(e => e.model))], nextCursor: start + 50 < filtered.length ? { id: last.id, time: last.time, revision: state.revision } : null };
        if (state.delay) { state.delay = false; await new Promise(resolve => state.release = resolve); }
        return structuredClone(result);
      }
      return null;
    } };
  }, fixture);
  await page.goto(baseURL);
  await page.getByRole('button', { name: '用量', exact: true }).click();
  await page.getByText('已加载 50 / 110 条', { exact: true }).waitFor();
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
  const afterExpand = await table.locator('th').nth(5).boundingBox();
  assert.equal(afterExpand.width, beforeExpand.width, 'expanded metadata must not resize status columns');
  await codex.locator('.request-expand').click();
  const project = table.locator('.request-project > small').filter({ hasText: 'example-project' });
  assert.equal(await project.getAttribute('title'), longProject);
  assert.ok((await project.textContent()).startsWith('~/Workspace/'));
  assert.equal(await table.locator('.request-project > span').filter({ hasText: /^\.codex$/ }).count(), 1);
  console.log(`PASS: actual client is preferred, raw source remains in details, project names and full paths remain available (${engine})`);
  await table.getByRole('button', { name: '加载更多', exact: true }).click();
  await page.getByText('已加载 100 / 110 条', { exact: true }).waitFor();
  await table.getByRole('button', { name: '加载更多', exact: true }).click();
  await page.getByText('已加载 110 / 110 条', { exact: true }).waitFor();
  assert.equal(await table.getByRole('button', { name: '加载更多', exact: true }).count(), 0);
  console.log(`PASS: real Rust fixture renders 110 events with keyset paging, approximate TPS and turn TTFT (${engine})`);

  await page.getByLabel('请求 Agent', { exact: true }).selectOption('claude');
  await page.getByText('已加载 50 / 109 条', { exact: true }).waitFor();
  await page.getByLabel('请求结果', { exact: true }).selectOption('failed');
  await page.getByText('已加载 1 / 1 条', { exact: true }).waitFor();
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
  await page.getByLabel('请求模型', { exact: true }).selectOption('claude-sonnet');
  await page.getByText('已加载 1 / 1 条', { exact: true }).waitFor();
  await table.getByRole('button', { name: '清除筛选', exact: true }).click();
  await page.getByText('已加载 50 / 110 条', { exact: true }).waitFor();
  assert.equal(await page.getByLabel('统计 Agent', { exact: true }).inputValue(), 'all');
  console.log(`PASS: shared Agent/model filters, results, details, unknown vs zero, and copying full session IDs (${engine})`);

  await page.evaluate(() => { window.__EVENTS__.revision = '2'; });
  await table.getByRole('button', { name: '加载更多', exact: true }).click();
  await page.getByText('检测到新事件，已更新到最新一页', { exact: true }).waitFor();
  assert.equal(await table.locator('tbody > tr').count(), 50);
  await page.evaluate(() => { window.__EVENTS__.delay = true; });
  await table.getByRole('button', { name: '加载更多', exact: true }).click();
  await page.waitForFunction(() => !!window.__EVENTS__.release);
  await page.getByLabel('请求 Agent', { exact: true }).selectOption('codex');
  await page.getByText('已加载 1 / 1 条', { exact: true }).waitFor();
  await page.evaluate(() => window.__EVENTS__.release());
  await page.waitForTimeout(50);
  assert.equal(await table.locator('tbody > tr').count(), 1);
  await page.evaluate(() => { window.__EVENTS__.fail = true; });
  await page.getByLabel('请求 Agent', { exact: true }).selectOption('all');
  await table.getByRole('alert').waitFor();
  await page.evaluate(() => { window.__EVENTS__.fail = false; });
  await table.getByRole('button', { name: '重试', exact: true }).click();
  await page.getByText('已加载 50 / 110 条', { exact: true }).waitFor();
  console.log(`PASS: changing data resets stale cursors, changing filters rejects late pages, read errors retry (${engine})`);

  for (const width of [960, 1280, 1600, 2200]) {
    await page.setViewportSize({ width, height: 1080 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    assert.ok(await table.locator('.request-table-wrap').evaluate(el => el.scrollWidth >= el.clientWidth && el.clientWidth > 450));
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
