import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import path from 'node:path';

const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const errors = []; page.on('pageerror', error => errors.push(error.message));
try {
  await page.clock.install({ time: new Date('2026-10-28T12:00:00Z') });
  await page.clock.pauseAt(new Date('2026-10-28T12:00:01Z'));
  await page.addInitScript(() => {
    const rows = ['first', 'second'].map(sessionId => ({ sessionId, title: `${sessionId} session`, projectDir: 'project', cwd: '/tmp/project',
      model: null, firstTs: null, lastTs: null, userCount: 1, assistantCount: 1, realUserCount: 1, assistantTextCount: 1, sizeBytes: 100, hasTranscript: true }));
    const state = window.__FEEDBACK__ = { rows, catalogs: 0, holdCatalog: false, failCatalog: false, releaseCatalog: null, copyMode: 'success', copied: [], releaseCopy: null, holdSync: false, releaseSync: null, total: 0, listeners: {}, listenerId: 0, watch: { generation: 0, agent: null } };
    state.emit = (event, payload) => {
      for (const [id, registration] of Object.entries(state.listeners)) if (registration.event === event) registration.handler({ event, id: Number(id), payload });
    };
    state.changed = () => state.emit('session-files-changed', state.watch);
    window.isTauri = true;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (command, args = {}) => {
      if (command === 'plugin:event|listen') { const id = ++state.listenerId; state.listeners[id] = args; return id; }
      if (command === 'plugin:event|unlisten') { delete state.listeners[args.eventId]; return null; }
      if (command === 'watch_sessions') { if (args.generation >= state.watch.generation) state.watch = args; return null; }
      if (command === 'list_agents') return [{ id: 'claude', displayName: 'Claude Code', installed: true, supportsFork: false, supportsModelNames: false, supportsFastCatalog: false, deletePolicy: 'trash' }];
      if (command === 'catalog') {
        ++state.catalogs;
        if (state.holdCatalog) await new Promise(resolve => state.releaseCatalog = resolve);
        if (state.failCatalog) throw Error('Catalog unavailable');
        return { sessions: structuredClone(rows), projects: [{ dirName: 'project', displayPath: '/tmp/project', sessionCount: 2, lastActivity: null }], warnings: [] };
      }
      if (command === 'read_session') return { meta: rows.find(row => row.sessionId === args.sessionId), events: [] };
      if (command === 'plugin:clipboard-manager|write_text') {
        if (state.copyMode === 'failure') throw Error('Clipboard unavailable');
        if (state.copyMode === 'pending') await new Promise(resolve => state.releaseCopy = resolve);
        state.copied.push(args.text);
        return;
      }
      if (command === 'collector_status') return { desired: false, running: false, authorization: 'signingRequired' };
      if (command === 'sync_request_events') {
        if (state.holdSync) await new Promise(resolve => state.releaseSync = resolve);
        return { updatedAt: Date.now(), checkedFiles: 0, changedFiles: 0, bytesRead: 0 };
      }
      if (command === 'read_usage_summary') return { rows: state.total ? [{ agent: 'claude', model: 'fixture', modelName: 'fixture', date: '', bucketStart: args.query.bounds[0] ?? null,
        inputTokens: state.total, outputTokens: 0, totalTokens: state.total, cacheReadTokens: 0, cacheWriteTokens: 0, reasoningTokens: 0, unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true, cacheReadRecorded: false, cacheWriteRecorded: false, reasoningRecorded: false, partial: false }] : [], health: {}, revision: '1' };
      if (command === 'read_request_events') return { events: [], models: [], total: 0, nextCursor: null, reset: false, revision: '1' };
      return null;
    } };
  });
  await page.goto(baseURL);
  await page.locator('.session-card', { hasText: 'first session' }).click();
  await page.evaluate(() => window.__FEEDBACK__.emit('session-window-focus', true));
  const copy = page.locator('.session-id');
  const size = async () => copy.evaluate(el => ({ width: el.getBoundingClientRect().width, height: el.getBoundingClientRect().height }));
  const originalSize = await size();
  await copy.click();
  await copy.locator('.lucide-check').waitFor();
  assert.deepEqual(await size(), originalSize, 'success must not change the button dimensions');
  assert.deepEqual(await page.evaluate(() => window.__FEEDBACK__.copied), ['first']);
  assert.equal(await page.locator('.toast').count(), 0, 'small copy buttons should not produce success toasts');
  await page.clock.runFor(1500);
  assert.equal(await copy.getAttribute('data-copied'), 'true');
  await copy.click();
  await page.clock.runFor(600);
  assert.equal(await copy.getAttribute('data-copied'), 'true', 'copying again restarts feedback duration');
  await page.clock.runFor(1450);
  await copy.locator('.lucide-copy').waitFor();
  assert.deepEqual(await size(), originalSize);

  await copy.click();
  await copy.locator('.lucide-check').waitFor();
  await page.clock.runFor(1500);
  await page.evaluate(() => { window.__FEEDBACK__.copyMode = 'pending'; });
  await copy.click();
  await page.clock.runFor(600);
  assert.equal(await copy.getAttribute('data-copied'), 'false', 'a pending write must not prolong the previous success feedback');
  await page.evaluate(() => window.__FEEDBACK__.releaseCopy());
  await copy.locator('.lucide-check').waitFor();
  await page.clock.runFor(2050);
  await copy.locator('.lucide-copy').waitFor();

  await page.evaluate(() => { window.__FEEDBACK__.copyMode = 'failure'; });
  await copy.click(); await page.locator('.toast.error').waitFor();
  assert.equal(await copy.getAttribute('data-copied'), 'false');
  await page.evaluate(() => { window.__FEEDBACK__.copyMode = 'pending'; });
  await copy.click();
  assert.equal(await copy.getAttribute('data-copied'), 'false', 'feedback must wait for clipboard success');
  await page.locator('.session-card', { hasText: 'second session' }).click();
  await page.getByRole('button', { name: '复制会话 ID：second', exact: true }).waitFor();
  await page.evaluate(() => window.__FEEDBACK__.releaseCopy());
  await page.clock.runFor(100);
  assert.equal(await copy.getAttribute('data-copied'), 'false', 'late results must not mark another session as copied');
  console.log(`PASS: inline copy feedback, stable dimensions, expiry/repeat, failure, and late clipboard results (${engine})`);

  assert.equal(await page.getByRole('button', { name: '刷新', exact: true }).count(), 0);
  await page.evaluate(() => { const s = window.__FEEDBACK__; s.holdCatalog = true; s.changed(); });
  await page.clock.fastForward(6000);
  await page.locator('.session-list[aria-busy="true"]').waitFor();
  await page.clock.runFor(400);
  assert.equal(await page.locator('.topbar .usage-spin').count(), 0);
  assert.equal(await page.locator('.session-card').count(), 2, 'background updates preserve existing sessions');
  await page.evaluate(() => { const s = window.__FEEDBACK__; s.rows[0].title = 'refreshed session'; s.holdCatalog = false; s.releaseCatalog(); });
  await page.locator('.session-card', { hasText: 'refreshed session' }).waitFor();
  await page.evaluate(() => { const s = window.__FEEDBACK__; s.failCatalog = true; s.changed(); });
  await page.clock.fastForward(12000);
  await page.locator('.catalog-error').waitFor();
  assert.equal(await page.locator('.session-card').count(), 2);
  await page.evaluate(() => { window.__FEEDBACK__.failCatalog = false; });
  await page.getByRole('button', { name: '重试读取', exact: true }).click();
  await page.locator('.session-list[aria-busy="false"]').waitFor();
  assert.equal(await page.locator('.catalog-error').count(), 0);
  console.log(`PASS: file changes update sessions without a refresh button; failures preserve data and offer a contextual retry (${engine})`);

  await page.evaluate(() => { window.__FEEDBACK__.holdSync = true; });
  await page.getByRole('button', { name: '用量', exact: true }).click();
  await page.evaluate(() => window.__FEEDBACK__.emit('session-window-focus', true));
  await page.getByText('正在读取历史请求…', { exact: true }).waitFor();
  await page.clock.runFor(100);
  assert.equal(await page.locator('.usage-sync-status,.usage-spin').count(), 0);
  await page.clock.runFor(250);
  await page.locator('.usage-sync-status').waitFor();
  await page.emulateMedia({ reducedMotion: 'reduce' });
  assert.equal(await page.locator('.usage-sync-status .usage-spin').evaluate(el => getComputedStyle(el).animationName), 'none');
  await page.evaluate(() => { const s = window.__FEEDBACK__; s.total = 100; s.holdSync = false; s.releaseSync(); });
  await page.locator('.usage-stat strong').first().filter({ hasText: /^100$/ }).waitFor();
  await page.clock.runFor(400);
  assert.equal(await page.locator('.usage-sync-status,.usage-spin').count(), 0);

  for (const total of [100, 0]) {
    if (total === 0) {
      await page.evaluate(() => { window.__FEEDBACK__.total = 0; });
      await page.getByRole('button', { name: '今日', exact: true }).click();
      await page.locator('.usage-stat strong').first().filter({ hasText: /^0$/ }).waitFor();
    }
    await page.evaluate(() => { const s = window.__FEEDBACK__; s.releaseSync = null; s.holdSync = true; s.changed(); });
    await page.clock.fastForward(12000);
    assert.ok(await page.evaluate(() => !!window.__FEEDBACK__.releaseSync), 'file changes must start another sync');
    await page.clock.runFor(500);
    assert.equal(await page.locator('.usage-stat strong').first().textContent(), String(total));
    assert.equal(await page.locator('.usage-sync-status,.usage-spin').count(), 0, 'warm updates, including empty ranges, should stay silent');
    await page.evaluate(() => { const s = window.__FEEDBACK__; s.total = 200; s.holdSync = false; s.releaseSync(); });
    await page.locator('.usage-stat strong').first().filter({ hasText: /^200$/ }).waitFor();
  }
  assert.equal(await page.getByRole('button', { name: '刷新', exact: true }).count(), 0);
  assert.equal(await page.locator('.usage-spin').count(), 0);
  assert.deepEqual(errors, []);
  console.log(`PASS: only the initial import shows delayed progress; subsequent file updates quietly replace usage totals (${engine})`);
} catch (error) {
  await page.screenshot({ path: path.join(artifactDir, `interaction-feedback-failure-${engine}.png`) }).catch(() => {});
  throw error;
} finally { await browser.close(); }
