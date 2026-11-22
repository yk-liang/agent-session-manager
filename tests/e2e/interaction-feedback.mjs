import { addUsageInitScript } from '../helpers/usage-collector.mjs';
import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import path from 'node:path';

const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, timezoneId: 'UTC' });
const errors = []; page.on('pageerror', error => errors.push(error.message));
try {
  await page.clock.install({ time: new Date('2026-10-28T12:00:00Z') });
  await page.clock.pauseAt(new Date('2026-10-28T12:00:01Z'));
  await addUsageInitScript(page, (_, createCollector) => {
    const rows = [
      { sessionId: 'first', cwd: '/tmp/project' },
      { sessionId: 'second', cwd: '/private/tmp/workspace/very-long-directory-name-for-overflow-check' },
    ].map(({ sessionId, cwd }) => ({ sessionId, title: `${sessionId} session`, projectDir: 'project', cwd,
      model: null, firstTs: null, lastTs: null, userCount: 1, assistantCount: 1, realUserCount: 1, assistantTextCount: 1, sizeBytes: 100, hasTranscript: true }));
    const state = window.__FEEDBACK__ = { rows, catalogs: 0, holdCatalog: false, failCatalog: false, releaseCatalog: null, copyMode: 'success', copied: [], releaseCopy: null, holdSync: false, releaseSync: null, total: 0, listeners: {}, listenerId: 0, watch: { generation: 0, agent: null } };
    state.emit = (event, payload) => {
      for (const [id, registration] of Object.entries(state.listeners)) if (registration.event === event) registration.handler({ event, id: Number(id), payload });
    };
    state.changed = () => state.emit('session-files-changed', state.watch);
    window.isTauri = true;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    state.collector = createCollector(['claude'], async () => {
      if (state.holdSync) await new Promise(resolve => state.releaseSync = resolve);
      state.synced = true;
      return true;
    });
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: state.collector.bridge(async (command, args = {}) => {
      if (command === 'plugin:event|listen') { const id = ++state.listenerId; state.listeners[id] = args; return id; }
      if (command === 'plugin:event|unlisten') { delete state.listeners[args.eventId]; return null; }
      if (command === 'watch_sessions') { if (args.generation >= state.watch.generation) state.watch = args; return null; }
      if (command === 'list_agents') return [{ id: 'claude', displayName: 'Claude Code', installed: true, supportsFork: false, deletePolicy: 'trash' }];
      if (command === 'catalog') {
        ++state.catalogs;
        if (state.holdCatalog) await new Promise(resolve => state.releaseCatalog = resolve);
        if (state.failCatalog) throw Error('Catalog unavailable');
        return { sessions: structuredClone(rows), projects: [{ dirName: 'project', displayPath: '/tmp/project', sessionCount: 2, lastActivity: null }], warnings: [] };
      }
      if (command === 'read_session') return { meta: rows.find(row => row.sessionId === args.sessionId), events: [
        { type: 'user', timestamp: '2026-10-28T09:07:05Z', message: { content: '用户的提问' } },
        { type: 'assistant', timestamp: '2026-10-28T09:08:06Z', message: {
          content: [{ type: 'text', text: '# 回答\n' + '原文'.repeat(1800) },
            { type: 'thinking', thinking: '不属于回复正文' }, { type: 'text', text: '第二段' }],
        } },
      ] };
      if (command === 'plugin:clipboard-manager|write_text') {
        if (state.copyMode === 'failure') throw Error('Clipboard unavailable');
        if (state.copyMode === 'pending') await new Promise(resolve => state.releaseCopy = resolve);
        state.copied.push(args.text);
        return;
      }
      if (command === 'read_usage_summary') return { rows: state.total ? [{ agent: 'claude', model: 'fixture', date: '', bucketStart: args.query.bounds[0] ?? null,
        inputTokens: state.total, outputTokens: 0, totalTokens: state.total, cacheReadTokens: 0, cacheWriteTokens: 0, reasoningTokens: 0, unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true, cacheReadRecorded: false, cacheWriteRecorded: false, reasoningRecorded: false, partial: false }] : [], syncedAgents: state.synced ? ['claude'] : [], revision: '1' };
      if (command === 'read_request_events') return { events: [], total: 0, page: 1, pageSize: args.query.limit ?? 50, revision: '1' };
      return null;
    }) };
  });
  await page.goto(baseURL);
  const secondCard = page.locator('.session-card', { hasText: 'second session' });
  const secondPath = secondCard.locator('.session-project-path');
  const secondPathIcon = secondPath.locator('.session-path-copy-icon');
  await secondCard.locator('.session-card-title').hover();
  assert.equal(await secondPathIcon.isVisible(), false, 'hovering the card must not show the path copy icon');
  const pathColor = await secondPath.evaluate(el => getComputedStyle(el).color);
  await secondPath.hover();
  assert.equal(await secondPath.locator('.lucide-copy').isVisible(), true);
  assert.equal(await secondPath.evaluate(el => getComputedStyle(el).color), pathColor, 'hovering the path tag should not highlight its text');
  const bounds = await secondCard.evaluate(card => ({
    cardRight: card.getBoundingClientRect().right,
    pathRight: card.querySelector('.session-project-path').getBoundingClientRect().right,
  }));
  assert.ok(bounds.pathRight <= bounds.cardRight - 10, `hovering a long path tag must not overflow the card: ${JSON.stringify(bounds)}`);
  await secondPath.click();
  await page.mouse.move(0, 0);
  await secondPath.locator('.lucide-check').waitFor();
  assert.deepEqual(await page.evaluate(() => window.__FEEDBACK__.copied), ['/private/tmp/workspace/very-long-directory-name-for-overflow-check']);
  assert.equal(await page.locator('.detail-title').count(), 0, 'copying a card path should not open the session');
  await page.clock.runFor(2100);
  await secondPathIcon.waitFor({ state: 'hidden' });
  await page.locator('.session-card', { hasText: 'first session' }).click();
  await page.evaluate(() => window.__FEEDBACK__.emit('session-window-focus', true));
  await page.getByRole('button', { name: '更多信息', exact: true }).click();
  const copyCwd = page.getByRole('button', { name: '复制工作目录', exact: true });
  await copyCwd.click();
  await copyCwd.locator('.lucide-check').waitFor();
  assert.equal(await page.evaluate(() => window.__FEEDBACK__.copied.at(-1)), '/tmp/project');
  await page.clock.runFor(2100);
  await copyCwd.locator('.lucide-copy').waitFor();
  await page.keyboard.press('Escape');
  const copy = page.locator('.session-id');
  const size = async () => copy.evaluate(el => ({ width: el.getBoundingClientRect().width, height: el.getBoundingClientRect().height }));
  const originalSize = await size();
  await copy.click();
  await copy.locator('.lucide-check').waitFor();
  assert.deepEqual(await size(), originalSize, 'success must not change the button dimensions');
  assert.equal(await page.evaluate(() => window.__FEEDBACK__.copied.at(-1)), 'first');
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

  await page.evaluate(() => { window.__FEEDBACK__.copyMode = 'success'; });
  const userMessage = page.locator('.message.user');
  const agentBubbles = page.locator('.message.assistant');
  const agentMessage = agentBubbles.last();
  const footer = (message) => message.locator('.message-actions');
  const reservedHeight = await userMessage.evaluate(el => el.offsetHeight - el.querySelector('.msg').offsetHeight);
  assert.ok(reservedHeight >= 24);
  assert.equal(await footer(userMessage).evaluate(el => getComputedStyle(el).opacity), '0');
  await userMessage.locator('.msg').hover();
  assert.equal(await footer(userMessage).evaluate(el => getComputedStyle(el).opacity), '1');
  assert.equal(await footer(userMessage).locator('time').textContent(), '2026-10-28 09:07:05');
  await userMessage.getByRole('button', { name: '复制消息', exact: true }).click();
  assert.equal(await page.evaluate(() => window.__FEEDBACK__.copied.at(-1)), '用户的提问');
  const copyMessage = agentMessage.getByRole('button', { name: '复制消息', exact: true });
  await copyMessage.focus();
  await copyMessage.press('Enter');
  await copyMessage.locator('.lucide-check').waitFor();
  assert.equal(await footer(agentMessage).locator('time').textContent(), '2026-10-28 09:08:06');
  assert.equal(await page.evaluate(() => window.__FEEDBACK__.copied.at(-1)), '第二段');
  await agentBubbles.first().getByRole('button', { name: '复制消息', exact: true }).press('Enter');
  await page.waitForFunction(() => window.__FEEDBACK__.copied.at(-1) === '# 回答\n' + '原文'.repeat(1800));
  await page.clock.runFor(2100);
  await copyMessage.locator('.lucide-copy').waitFor();
  assert.equal(await userMessage.evaluate(el => el.offsetHeight - el.querySelector('.msg').offsetHeight), reservedHeight);

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
  await page.getByRole('alert', { name: '无法读取会话列表', exact: true }).waitFor();
  assert.equal(await page.locator('.session-card').count(), 2);
  await page.evaluate(() => { window.__FEEDBACK__.failCatalog = false; });
  await page.getByRole('button', { name: '重试', exact: true }).click();
  await page.locator('.session-list[aria-busy="false"]').waitFor();
  assert.equal(await page.getByRole('alert', { name: '无法读取会话列表', exact: true }).count(), 0);
  console.log(`PASS: file changes update sessions without a refresh button; failures preserve data and offer a contextual retry (${engine})`);

  await page.evaluate(() => { const state = window.__FEEDBACK__; state.holdSync = true; void state.collector.check(); });
  await page.getByRole('button', { name: '用量', exact: true }).click();
  await page.evaluate(() => window.__FEEDBACK__.emit('session-window-focus', true));
  await page.locator('.usage-chart-empty').getByText('正在同步历史用量…', { exact: true }).waitFor();
  await page.clock.runFor(100);
  assert.equal(await page.locator('.usage-coverage,.usage-spin').count(), 0);
  await page.clock.runFor(250);
  await page.locator('.usage-coverage').waitFor();
  await page.emulateMedia({ reducedMotion: 'reduce' });
  assert.equal(await page.locator('.usage-coverage .usage-spin').evaluate(el => getComputedStyle(el).animationName), 'none');
  await page.evaluate(() => { const s = window.__FEEDBACK__; s.total = 100; s.holdSync = false; s.releaseSync(); });
  await page.locator('.usage-stat[data-metric="tokens"] strong').filter({ hasText: /^100$/ }).waitFor();
  await page.clock.runFor(400);
  assert.equal(await page.locator('.usage-coverage,.usage-spin').count(), 0);

  for (const total of [100, 0]) {
    if (total === 0) {
      await page.evaluate(() => { window.__FEEDBACK__.total = 0; });
      await page.getByRole('button', { name: '今日', exact: true }).click();
      await page.locator('.usage-stat[data-metric="tokens"] strong').filter({ hasText: /^0$/ }).waitFor();
    }
    await page.evaluate(() => { const s = window.__FEEDBACK__; s.releaseSync = null; s.holdSync = true; void s.collector.check(); });
    await page.waitForFunction(() => !!window.__FEEDBACK__.releaseSync);
    assert.ok(await page.evaluate(() => !!window.__FEEDBACK__.releaseSync), 'file changes must start another sync');
    await page.clock.runFor(500);
    assert.equal(await page.locator('.usage-stat[data-metric="tokens"] strong').textContent(), String(total));
    assert.equal(await page.locator('.usage-coverage,.usage-spin').count(), 0, 'warm updates, including empty ranges, should stay silent');
    await page.evaluate(() => { const s = window.__FEEDBACK__; s.total = 200; s.holdSync = false; s.releaseSync(); });
    await page.locator('.usage-stat[data-metric="tokens"] strong').filter({ hasText: /^200$/ }).waitFor();
  }
  assert.equal(await page.getByRole('button', { name: '刷新', exact: true }).count(), 0);
  assert.equal(await page.locator('.usage-spin').count(), 0);
  assert.deepEqual(errors, []);
  console.log(`PASS: only the initial import shows delayed progress; subsequent file updates quietly replace usage totals (${engine})`);
} catch (error) {
  await page.screenshot({ path: path.join(artifactDir, `interaction-feedback-failure-${engine}.png`) }).catch(() => {});
  throw error;
} finally { await browser.close(); }
