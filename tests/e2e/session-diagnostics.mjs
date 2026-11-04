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
  await page.addInitScript(() => {
    Object.defineProperty(document, 'hasFocus', { value: () => false });
    const row = id => ({ sessionId: id, projectDir: 'project', title: `Cached ${id}`, model: null, cwd: '/synthetic/project', realUserCount: 1, assistantTextCount: 1, hasTranscript: true, sizeBytes: 123, firstTs: null, lastTs: null });
    const state = window.__DIAGNOSTICS__ = { catalogMode: new URL(location.href).searchParams.get('catalog'), readable: [], copied: '', contentBroken: null, partial: false, syncFails: true, hold: null, release: null, listeners: new Map() };
    const failure = (operation, id = 'aaaa', code = 24) => `会话文件访问失败：/synthetic/project/${id}.jsonl\noperation: ${operation}\nstage: open\nerror_kind: ${code === 13 || code === 1 ? 'PermissionDenied' : 'Uncategorized'}\nos_code: ${code}\ncause: ${code === 1 ? 'Operation not permitted' : code === 13 ? 'Permission denied' : 'Too many open files'} (os error ${code})\nprocess: gui\npid: 1234\napp_version: 0.1.39`;
    const syntaxError = '会话内容无法解析：/synthetic/project/bbbb.jsonl（第 2 行）\noperation: session_detail\nstage: parse_json\nerror_kind: JsonSyntax\nos_code: none\ncause: trailing comma at line 1 column 8\nline: 2\ncolumn: 8';
    state.failure = failure;
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (command, args = {}) => {
      if (command === 'list_agents') return ['qoder', 'claude'].map(id => ({ id, displayName: id === 'qoder' ? 'Qoder CLI' : 'Claude Code', installed: true, supportsFork: false, deletePolicy: 'trash' }));
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
      if (command === 'resolve_model_names') return {};
      if (command === 'plugin:clipboard-manager|write_text') { state.copied = args.text; return null; }
      if (command === 'plugin:event|listen') { state.listeners.set(args.event, args.handler); return 1; }
      if (command === 'search_conversations') return { hits: [], skipped: 0, cancelled: false };
      if (command === 'sync_request_events') {
        if (state.syncFails && args.agent === 'qoder') throw failure('usage_sync', 'aaaa', 13);
        return { updatedAt: Date.now(), checkedFiles: 2, changedFiles: 0, bytesRead: 0 };
      }
      if (command === 'read_usage_summary') return { rows: [], health: {}, syncedAgents: ['qoder', 'claude'], revision: '1' };
      if (command === 'read_request_events') return { events: [], models: [], total: 0, page: 1, pageSize: 50, revision: '1' };
      if (command === 'collector_status') return { desired: false, running: false, authorization: 'disabled' };
      return null;
    } };
  });
  const card = id => page.locator('.session-card').filter({ hasText: `Cached ${id}` });
  const errorPanel = page.locator('.session-read-error');
  const failedCards = page.locator('.session-card .read-error-badge');
  const copied = () => page.evaluate(() => window.__DIAGNOSTICS__.copied);

  await page.goto(baseURL);
  await card('aaaa').click();
  await errorPanel.waitFor();
  assert.equal(await page.locator('.session-card').count(), 2);
  assert.equal(await failedCards.count(), 1);
  assert.match(await errorPanel.textContent(), /阶段：open.*系统码：24/s);
  await errorPanel.getByRole('button', { name: '复制诊断详情', exact: true }).click();
  await page.waitForFunction(() => window.__DIAGNOSTICS__.copied.includes('operation: session_detail'));
  assert.equal(await copied(), await page.evaluate(() => window.__DIAGNOSTICS__.failure('session_detail')));
  await card('bbbb').click();
  await page.waitForFunction(() => document.querySelectorAll('.read-error-badge').length === 2);
  await page.locator('.catalog-warnings').getByText('本次扫描有 1 条告警', { exact: true }).waitFor();
  assert.match(await page.locator('.catalog-warnings').textContent(), /告警条数不代表全部不可读会话/);
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
  await page.locator('.catalog-warnings').getByRole('button', { name: '重试扫描', exact: true }).click();
  await page.locator('.catalog-warnings').getByRole('button', { name: '重试扫描', exact: true }).waitFor();
  assert.equal(await failedCards.count(), 2, 'successful cached catalog refresh must not erase body failures');
  await card('aaaa').click();
  await errorPanel.waitFor();
  await page.evaluate(() => window.__DIAGNOSTICS__.readable.push('aaaa'));
  await errorPanel.getByRole('button', { name: '重试读取', exact: true }).click();
  await page.getByText('Recovered aaaa', { exact: true }).waitFor();
  assert.equal(await failedCards.count(), 1, 'successful retry clears only its own failure');
  console.log(`PASS: cached summaries retain multiple body failures; scan warning count, exact copy, Agent isolation and retry are correct (${engine})`);

  await page.evaluate(() => { window.__DIAGNOSTICS__.contentBroken = 'bbbb'; });
  await card('bbbb').click();
  await errorPanel.getByText('会话内容解析失败', { exact: true }).waitFor();
  assert.match(await errorPanel.textContent(), /阶段：parse_json.*系统码：none/s);
  await page.evaluate(() => { const s = window.__DIAGNOSTICS__; s.contentBroken = null; s.readable.push('bbbb'); s.partial = true; });
  await errorPanel.getByRole('button', { name: '重试读取', exact: true }).click();
  await page.getByText('Recovered bbbb', { exact: true }).waitFor();
  await page.locator('.detail-warnings').getByRole('button', { name: '复制诊断详情', exact: true }).click();
  await page.waitForFunction(() => window.__DIAGNOSTICS__.copied.includes('stage: parse_json'));
  assert.match(await copied(), /line: 2/);
  await page.evaluate(() => { window.__DIAGNOSTICS__.partial = false; });
  await page.locator('.detail-warnings').getByRole('button', { name: '重试读取', exact: true }).click();
  await page.getByText('Recovered bbbb', { exact: true }).waitFor();
  assert.equal(await page.locator('.detail-warnings').count(), 0);
  console.log(`PASS: invalid content has a distinct error; partial content keeps a copyable warning and can recover (${engine})`);

  await page.getByRole('button', { name: '用量', exact: true }).click();
  const usageError = page.locator('.usage-sync-error');
  await usageError.waitFor();
  assert.match(await usageError.textContent(), /已保存记录仍可查看/);
  assert.match(await usageError.textContent(), /阶段：open.*类型：PermissionDenied.*系统码：13/s);
  await usageError.getByRole('button', { name: '复制诊断详情', exact: true }).click();
  await page.waitForFunction(() => window.__DIAGNOSTICS__.copied.includes('operation: usage_sync'));
  assert.match(await copied(), /cause: Permission denied \(os error 13\)/);
  await usageError.locator('summary').click();
  await page.screenshot({ path: path.join(artifactDir, 'usage-read-diagnostics.png') });
  await page.evaluate(() => { window.__DIAGNOSTICS__.syncFails = false; });
  await usageError.getByRole('button', { name: '重试同步', exact: true }).click();
  await usageError.waitFor({ state: 'detached' });
  console.log(`PASS: usage sync exposes the original GUI diagnostic and clears it after a successful retry (${engine})`);

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
  await page.locator('.catalog-warnings').getByText('本次扫描有 283 条告警', { exact: true }).waitFor();
  assert.doesNotMatch(await page.locator('.sidebar').textContent(), /0 个会话|未发现会话/, 'failed reads must not imply an empty history');
  assert.match(await page.locator('.sidebar-projects-header').textContent(), /会话数量暂不可确认/);
  assert.match(await page.locator('.session-cards').textContent(), /会话列表读取不完整/);
  assert.doesNotMatch(await page.locator('.session-cards').textContent(), /没有匹配的会话/);
  await page.setViewportSize({ width: 960, height: 600 });
  await page.screenshot({ path: path.join(artifactDir, 'catalog-all-unreadable.png') });

  await page.evaluate(() => { window.__DIAGNOSTICS__.catalogMode = 'recovered'; });
  await page.locator('.catalog-warnings').getByRole('button', { name: '重试扫描', exact: true }).click();
  await card('aaaa').waitFor();
  assert.equal(await page.locator('.catalog-warnings').count(), 0);
  assert.match(await page.locator('.sidebar-projects-header').textContent(), /2 个会话/);
  assert.doesNotMatch(await page.locator('.sidebar').textContent(), /暂不可确认|读取不完整/);

  await page.goto(`${baseURL}?catalog=empty`);
  await page.locator('.sidebar').getByText('未发现会话', { exact: true }).waitFor();
  assert.match(await page.locator('.sidebar-projects-header').textContent(), /0 个会话/);
  assert.equal(await page.locator('.catalog-warnings').count(), 0);
  assert.deepEqual(pageErrors, []);
  console.log(`PASS: denied cold-start reads remain distinct from empty history and recover after retry (${engine})`);
} finally { await browser.close(); }
