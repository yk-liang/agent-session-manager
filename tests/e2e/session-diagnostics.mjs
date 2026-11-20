import { addUsageInitScript } from '../helpers/usage-collector.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import path from 'node:path';
import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chooseAgent } from '../helpers/agent-switcher.mjs';

const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
const pageErrors = [];
page.on('pageerror', error => pageErrors.push(error.message));
try {
  await addUsageInitScript(page, (_, createCollector) => {
    Object.defineProperty(document, 'hasFocus', { value: () => false });
    const row = id => ({ sessionId: id, projectDir: 'project', title: `Cached ${id}`, model: null, cwd: '/synthetic/project', realUserCount: 1, assistantTextCount: 1, hasTranscript: true, sizeBytes: 123, firstTs: null, lastTs: null });
    const state = window.__DIAGNOSTICS__ = { catalogMode: new URL(location.href).searchParams.get('catalog'), readable: [], copied: '', contentBroken: null, partial: false, syncFails: true, hold: null, release: null, listeners: new Map() };
    const failure = (operation, id = 'aaaa', code = 24) => `会话文件访问失败：/synthetic/project/${id}.jsonl\noperation: ${operation}\nstage: open\nerror_kind: ${code === 13 || code === 1 ? 'PermissionDenied' : 'Uncategorized'}\nos_code: ${code}\ncause: ${code === 1 ? 'Operation not permitted' : code === 13 ? 'Permission denied' : 'Too many open files'} (os error ${code})\nprocess: gui\npid: 1234\napp_version: 0.1.39`;
    const syntaxError = '会话内容无法解析：/synthetic/project/bbbb.jsonl（第 2 行）\noperation: session_detail\nstage: parse_json\nerror_kind: JsonSyntax\nos_code: none\ncause: trailing comma at line 1 column 8\nline: 2\ncolumn: 8';
    state.failure = failure;
    state.collector = createCollector(['qoder', 'claude'], async agent => {
      if (state.syncFails && agent === 'qoder') throw failure('usage_sync', 'aaaa', 13);
      return false;
    });
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: state.collector.bridge(async (command, args = {}) => {
      if (command === 'list_agents') return ['qoder', 'claude'].map(id => ({ id, displayName: id === 'qoder' ? 'Qoder' : 'Claude Code', installed: true, supportsFork: false, deletePolicy: 'trash' }));
      if (command === 'catalog') {
        if (state.catalogMode === 'failed') return { sessions: [], projects: [], warnings: Array.from({ length: 283 }, (_, i) => failure('catalog_metadata', `session-${i}`, 1)) };
        if (state.catalogMode === 'empty') return { sessions: [], projects: [], warnings: [] };
        return { sessions: [row('aaaa'), row('bbbb')], projects: [{ dirName: 'project', displayPath: '/synthetic/project', sessionCount: 2 }], warnings: args.agent === 'qoder' && state.catalogMode !== 'recovered' ? [failure('catalog_metadata')] : [] };
      }
      if (command === 'read_session') {
        if (state.hold === args.sessionId) { await new Promise(resolve => state.release = resolve); throw failure('session_detail', args.sessionId); }
        if (state.contentBroken === args.sessionId) throw syntaxError;
        if (!state.readable.includes(args.sessionId)) throw failure('session_detail', args.sessionId);
        return { meta: row(args.sessionId), events: [{ type: 'user', message: { content: `Recovered ${args.sessionId}` } }], warnings: state.partial ? [syntaxError] : [] };
      }
      if (command === 'plugin:clipboard-manager|write_text') { state.copied = args.text; return null; }
      if (command === 'plugin:event|listen') { state.listeners.set(args.event, args.handler); return 1; }
      if (command === 'search_conversations') return { hits: [], skipped: 0, cancelled: false };
      if (command === 'read_usage_summary') return { rows: [], syncedAgents: ['qoder', 'claude'], revision: '1' };
      if (command === 'read_request_events') return { events: [], total: 0, page: 1, pageSize: 50, revision: '1' };
      return null;
    }) };
    void state.collector.check();
  });
  const card = id => page.locator('.session-card').filter({ hasText: `Cached ${id}` });
  const errorPanel = page.getByRole('alert', { name: /无法(?:解析|读取)会话正文/ });
  const catalogWarnings = page.getByRole('alert', { name: '部分会话无法读取', exact: true });
  const failedCards = page.locator('.session-card .status-tag[data-tone="error"]');
  const copied = () => page.evaluate(() => window.__DIAGNOSTICS__.copied);
  const recovered = id => page.locator('.detail-body').getByText(`Recovered ${id}`, { exact: true });

  await page.goto(baseURL);
  await card('aaaa').click();
  await errorPanel.waitFor();
  assert.equal(await page.locator('.session-card').count(), 2);
  assert.equal(await failedCards.count(), 1);
  assert.deepEqual(await errorPanel.locator('.read-error-fields dd').allTextContents(), ['open', 'Uncategorized', '24']);
  await errorPanel.getByRole('button', { name: '复制诊断详情', exact: true }).click();
  await page.waitForFunction(() => window.__DIAGNOSTICS__.copied.includes('operation: session_detail'));
  assert.equal(await copied(), await page.evaluate(() => window.__DIAGNOSTICS__.failure('session_detail')));
  await card('bbbb').click();
  await page.waitForFunction(() => document.querySelectorAll('.status-tag[data-tone="error"]').length === 2);
  await catalogWarnings.getByText('部分会话无法读取', { exact: true }).waitFor();
  assert.match(await catalogWarnings.textContent(), /部分会话无法读取/);
  for (const width of [960, 1280]) {
    await page.setViewportSize({ width, height: 800 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    assert.ok(await errorPanel.evaluate(el => el.scrollWidth <= el.clientWidth));
  }
  await errorPanel.locator('summary').click();
  await page.screenshot({ path: path.join(artifactDir, 'session-read-diagnostics.png') });
  await chooseAgent(page, 'claude');
  assert.equal(await failedCards.count(), 0);
  await chooseAgent(page, 'qoder');
  assert.equal(await failedCards.count(), 2);
  await catalogWarnings.getByRole('button', { name: '重试', exact: true }).click();
  await catalogWarnings.getByRole('button', { name: '重试', exact: true }).waitFor();
  assert.equal(await failedCards.count(), 2, 'successful cached catalog refresh must not erase body failures');
  await card('aaaa').click();
  await errorPanel.waitFor();
  await page.evaluate(() => window.__DIAGNOSTICS__.readable.push('aaaa'));
  await errorPanel.getByRole('button', { name: '重试', exact: true }).click();
  await recovered('aaaa').waitFor();
  assert.equal(await failedCards.count(), 1, 'successful retry clears only its own failure');
  console.log(`PASS: cached summaries retain multiple body failures; scan warning count, exact copy, Agent isolation and retry are correct (${engine})`);

  await page.evaluate(() => { window.__DIAGNOSTICS__.contentBroken = 'bbbb'; });
  await card('bbbb').click();
  await errorPanel.getByText('无法解析会话正文', { exact: true }).waitFor();
  assert.deepEqual(await errorPanel.locator('.read-error-fields dd').allTextContents(), ['parse_json', 'JsonSyntax', 'none']);
  await page.evaluate(() => { const s = window.__DIAGNOSTICS__; s.contentBroken = null; s.readable.push('bbbb'); s.partial = true; });
  await errorPanel.getByRole('button', { name: '重试', exact: true }).click();
  await recovered('bbbb').waitFor();
  await page.locator('.detail-warnings').getByRole('button', { name: '复制诊断详情', exact: true }).click();
  await page.waitForFunction(() => window.__DIAGNOSTICS__.copied.includes('stage: parse_json'));
  assert.match(await copied(), /line: 2/);
  await page.evaluate(() => { window.__DIAGNOSTICS__.partial = false; });
  await page.locator('.detail-warnings').getByRole('button', { name: '重试', exact: true }).click();
  await recovered('bbbb').waitFor();
  assert.equal(await page.locator('.detail-warnings').count(), 0);
  console.log(`PASS: invalid content has a distinct error; partial content keeps a copyable warning and can recover (${engine})`);

  await page.getByRole('button', { name: '用量', exact: true }).click();
  const syncFailure = page.getByRole('button', { name: '同步状态，同步失败' });
  await syncFailure.click();
  const syncPanel = page.getByRole('dialog', { name: '用量同步状态' });
  await syncPanel.getByText('读取失败，等待重试', { exact: true }).waitFor();
  assert.doesNotMatch(await syncPanel.textContent(), /诊断详情|Permission denied/);
  await page.evaluate(() => { window.__DIAGNOSTICS__.syncFails = false; });
  await syncPanel.getByRole('button', { name: '重试同步', exact: true }).click();
  await syncFailure.waitFor({ state: 'hidden' });
  console.log(`PASS: usage sync shows a concise failure and clears it after a successful retry (${engine})`);

  await page.getByRole('button', { name: '会话', exact: true }).click();
  await page.evaluate(() => { window.__DIAGNOSTICS__.hold = 'aaaa'; });
  await card('aaaa').click();
  await page.waitForFunction(() => window.__DIAGNOSTICS__.release !== null);
  await chooseAgent(page, 'claude');
  await page.evaluate(() => window.__DIAGNOSTICS__.release());
  await page.waitForTimeout(100);
  assert.equal(await errorPanel.count(), 0, 'late failure cannot replace the newly selected Agent');
  assert.equal(await failedCards.count(), 0);
  assert.deepEqual(pageErrors, []);
  console.log(`PASS: late failed reads cannot leak diagnostics into another Agent (${engine})`);

  // Replay the reported cold-start shape: every file fails open and no summary is available.
  await page.goto(`${baseURL}?catalog=failed`);
  await catalogWarnings.getByText('诊断详情（283 条）', { exact: true }).waitFor();
  assert.doesNotMatch(await page.locator('.sidebar').textContent(), /0 个会话|暂无会话/, 'failed reads must not imply an empty history');
  assert.match(await page.locator('.sidebar-projects-header').textContent(), /会话数量暂不可确认/);
  assert.match(await page.locator('.session-cards').textContent(), /暂无可显示的会话/);
  assert.doesNotMatch(await page.locator('.session-cards').textContent(), /暂无会话/);
  await page.setViewportSize({ width: 960, height: 600 });
  await page.screenshot({ path: path.join(artifactDir, 'catalog-all-unreadable.png') });

  await page.evaluate(() => { window.__DIAGNOSTICS__.catalogMode = 'recovered'; });
  await catalogWarnings.getByRole('button', { name: '重试', exact: true }).click();
  await card('aaaa').waitFor();
  assert.equal(await catalogWarnings.count(), 0);
  assert.match(await page.locator('.sidebar-projects-header').textContent(), /2 个会话/);
  assert.doesNotMatch(await page.locator('.sidebar').textContent(), /暂不可确认|读取不完整/);

  await page.goto(`${baseURL}?catalog=empty`);
  await page.locator('.sidebar').getByText('暂无会话', { exact: true }).waitFor();
  assert.match(await page.locator('.sidebar-projects-header').textContent(), /0 个会话/);
  assert.equal(await catalogWarnings.count(), 0);
  assert.deepEqual(pageErrors, []);
  console.log(`PASS: denied cold-start reads remain distinct from empty history and recover after retry (${engine})`);
} finally { await browser.close(); }
