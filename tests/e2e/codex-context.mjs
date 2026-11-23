import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const dir = mkdtempSync(path.join(tmpdir(), 'asm-codex-context-'));
const exported = path.join(dir, 'fixture.json');
let browser;
try {
  const run = await promisify(execFile)('cargo', ['test', '--offline', '--locked',
    'context_is_preserved_without_polluting_counts_titles_search_or_event_identity', '--', '--nocapture'], {
    cwd: path.resolve('src-tauri'), env: { ...process.env, ASM_CODEX_CONTEXT_EXPORT: exported }, timeout: 180000,
  });
  assert.match(run.stdout, /1 passed/);
  const fixture = JSON.parse(readFileSync(exported, 'utf8'));
  browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.addInitScript(fixture => {
    window.__CONTEXT_CALLS__ = [];
    window.__HIERARCHY__ = sessionStorage.getItem('codex-hierarchy') === 'true';
    const archived = new Set();
    let deleted = new Set();
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
      window.__CONTEXT_CALLS__.push({ cmd, args });
      if (cmd === 'list_agents') return [{ id: 'codex', displayName: 'Codex', installed: true, supportsArchive: true, supportsArchivedSessions: true, deletePolicy: 'permanent' }];
      if (cmd === 'catalog') {
        const catalog = structuredClone(args.includeEmpty ? fixture.all : fixture.catalog);
        if (window.__HIERARCHY__) {
          const original = catalog.sessions[0];
          const row = (sessionId, title, parentId, options = {}) => ({
            ...original, sessionId, title, archived: false,
            codex: { source: options.source ?? (parentId ? 'subagent' : 'exec'), canResume: !parentId, empty: false },
            subagent: parentId ? { parentId, parent: { sessionId: parentId, projectDir: original.projectDir }, orphan: options.orphan ?? false } : undefined,
          });
          catalog.sessions = [
            row('main', '父任务', undefined, { source: 'cli' }),
            row('child', '一级子任务', 'main'),
            row('leaf', '二级子任务', 'child'),
            row('orphan', '孤儿任务', 'missing', { orphan: true }),
            row('exec', '独立执行', undefined),
          ].filter(session => !deleted.has(session.sessionId));
          catalog.projects = [{ ...fixture.catalog.projects[0], sessionCount: catalog.sessions.length }];
          window.__HIERARCHY_ROWS__ = catalog.sessions;
          return catalog;
        }
        for (const session of catalog.sessions) session.archived = archived.has(session.sessionId);
        if (!args.includeArchived) catalog.sessions = catalog.sessions.filter(session => !session.archived);
        for (const project of catalog.projects) project.sessionCount = catalog.sessions.filter(session => session.projectDir === project.dirName).length;
        catalog.projects = catalog.projects.filter(project => project.sessionCount);
        return catalog;
      }
      if (cmd === 'read_session') {
        if (window.__HIERARCHY__) return {
          meta: window.__HIERARCHY_ROWS__.find(row => row.sessionId === args.sessionId),
          events: [
            ...(args.sessionId === 'leaf' ? [
              { type: 'user', message: { content: '父会话的提问' } },
              { type: 'assistant', message: { content: '父会话的回答' } },
            ] : []),
            { type: 'assistant', message: { content: '子会话正文样例' } },
          ],
          inherited: args.sessionId === 'leaf' ? 2 : 0,
        };
        const detail = structuredClone(fixture.details.find(d => d.meta.sessionId === args.sessionId));
        detail.meta.archived = archived.has(args.sessionId);
        return detail;
      }
      if (cmd === 'delete_session' && window.__HIERARCHY__) {
        deleted = new Set(['main', 'child', 'leaf']);
        const removed = window.__HIERARCHY_ROWS__.filter(row => deleted.has(row.sessionId))
          .map(({ sessionId, projectDir }) => ({ sessionId, projectDir }));
        return { removed, error: null };
      }
      if (cmd === 'set_session_archived') {
        if (args.archived) archived.add(args.sessionId);
        else archived.delete(args.sessionId);
        return null;
      }
      if (cmd === 'search_conversations') return args.query === 'visible needle' ? fixture.search : { hits: [], skipped: 0, cancelled: false };
      return null;
    } };
  }, fixture);
  await page.goto(baseURL);
  await page.locator('.session-card').click();
  await page.locator('.detail-body').getByText('First actual request', { exact: true }).waitFor();
  const context = page.getByRole('checkbox', { name: '上下文', exact: true });
  assert.equal(await context.isChecked(), false);
  assert.equal(await page.locator('.collapsible.context').count(), 0);
  assert.equal(await page.locator('.toc-item').count(), 2);
  assert.equal(await page.locator('.detail-body').getByText(/runtime-only-needle/).count(), 0);
  assert.ok((await page.locator('.detail-body').textContent()).includes('Second actual request: explain <external_codex_apps_open_page>'));
  await page.screenshot({ path: path.join(artifactDir, `codex-context-hidden-${engine}.png`) });
  console.log(`PASS: runtime instructions and Page state are hidden by default; real prompts and two-entry TOC remain (${engine})`);

  await context.check();
  const developer = page.getByRole('button', { name: '开发者指令', exact: true });
  await developer.waitFor();
  assert.equal(await developer.getAttribute('aria-expanded'), 'false');
  await developer.click();
  await page.locator('.collapsible.context .collapsible-body').getByText('Runtime instructions: runtime-only-needle', { exact: true }).waitFor();
  await developer.click();
  const pageState = page.locator('.collapsible.context').filter({ has: page.getByRole('button', { name: '页面上下文 未打开页面', exact: true }) }).first();
  await pageState.getByRole('button').click();
  assert.equal(await pageState.locator('.code').textContent(), '<external_codex_apps_open_page>{"page_id":null}</external_codex_apps_open_page>');
  await page.getByRole('checkbox', { name: 'Markdown', exact: true }).check();
  assert.ok((await pageState.locator('.code').textContent()).includes('<external_codex_apps_open_page>'));
  await pageState.getByRole('button').click();
  assert.equal(await page.locator('.toc-item').count(), 2);
  await page.waitForFunction(() => {
    const rows = [...document.querySelectorAll('.event-group')];
    return rows.every((row, i) => i === 0 || Math.abs(row.getBoundingClientRect().top - rows[i - 1].getBoundingClientRect().bottom - 8) < 1);
  });
  await page.screenshot({ path: path.join(artifactDir, `codex-context-expanded-${engine}.png`) });
  console.log(`PASS: optional context rows have readable Page summaries and preserve expandable original records (${engine})`);

  await context.uncheck();
  await page.getByRole('searchbox').fill('visible needle');
  await page.locator('.session-search-snippet').waitFor();
  await page.locator('.session-card').click();
  await page.locator('.search-target mark').waitFor();
  assert.equal(await page.locator('.search-target mark').textContent(), 'visible needle');
  assert.equal(await page.locator('.collapsible.context').count(), 0);
  await page.getByRole('searchbox').fill('runtime-only-needle');
  await page.waitForFunction(() => window.__CONTEXT_CALLS__.some(c => c.cmd === 'search_conversations' && c.args.query === 'runtime-only-needle')
    && document.querySelectorAll('.session-card').length === 0);
  console.log(`PASS: hidden context does not enter conversation search; result indices still locate the right answer (${engine})`);

  await page.getByRole('searchbox').fill('');
  await page.getByRole('checkbox', { name: '空会话', exact: true }).check();
  await page.waitForFunction(() => document.querySelectorAll('.session-card').length === 2);
  const index = fixture.all.sessions.findIndex(s => s.sessionId === 'context-only');
  await page.locator('.session-card').nth(index).click();
  await page.getByText('勾选「上下文」查看运行记录。', { exact: true }).waitFor();
  assert.equal(await page.locator('.toc-item').count(), 0);
  await context.check();
  await page.waitForFunction(() => document.querySelectorAll('.collapsible.context').length === 2);
  await page.setViewportSize({ width: 960, height: 760 });
  assert.ok(await page.locator('.detail-header').evaluate(el => el.scrollWidth <= el.clientWidth + 1));

  const showEmpty = page.getByRole('checkbox', { name: '空会话', exact: true });
  const showArchived = page.getByRole('checkbox', { name: '已归档', exact: true });
  const emptyBounds = await showEmpty.boundingBox();
  const archiveBounds = await showArchived.boundingBox();
  assert.ok(archiveBounds.x > emptyBounds.x && archiveBounds.y === emptyBounds.y);
  assert.equal(await showArchived.isChecked(), false);
  const archive = page.locator('.detail-actions').getByRole('button', { name: '归档', exact: true });
  assert.equal(await archive.locator('svg').count(), 0, '详情工具栏的归档按钮与同排文本按钮一致');
  await archive.click();
  const archivedCard = page.locator('.session-card.archived');
  await page.locator('.placeholder').waitFor();
  await showArchived.check();
  await archivedCard.locator('.archived-ribbon').getByText('已归档', { exact: true }).waitFor();
  await showEmpty.uncheck();
  await archivedCard.waitFor({ state: 'hidden' });
  await showEmpty.check();
  await archivedCard.getByRole('button', { name: /空会话/ }).first().click();
  await page.locator('.detail-header').getByText('已归档', { exact: true }).waitFor();
  await showArchived.uncheck();
  await page.locator('.placeholder').waitFor();
  await showArchived.check();
  await archivedCard.hover();
  const restore = archivedCard.getByRole('button', { name: /^取消归档/ });
  await restore.hover();
  await restore.getByText('取消归档', { exact: true }).waitFor();
  await restore.click();
  await archivedCard.waitFor({ state: 'hidden' });
  await showArchived.uncheck();
  await page.waitForFunction(() => document.querySelectorAll('.session-card').length === 2);
  const changes = await page.evaluate(() => window.__CONTEXT_CALLS__.filter(c => c.cmd === 'set_session_archived').map(c => [c.args.sessionId, c.args.archived]));
  assert.deepEqual(changes, [['context-only', true], ['context-only', false]]);
  await page.locator('.session-card-open').filter({ hasText: 'First actual request' }).click();
  await page.locator('.detail-actions').getByRole('button', { name: '删除', exact: true }).click();
  const confirm = page.getByRole('dialog');
  await confirm.getByText('删除后无法恢复。').waitFor();
  await confirm.getByText('子会话也会一并删除。').waitFor();
  await confirm.getByRole('button', { name: '永久删除', exact: true }).waitFor();
  await confirm.getByRole('button', { name: '取消', exact: true }).click();
  assert.deepEqual(errors, []);
  console.log(`PASS: archive controls, badges and independent empty/archive filters fit the minimum window width (${engine})`);

  await page.evaluate(() => sessionStorage.setItem('codex-hierarchy', 'true'));
  await page.reload();
  const card = title => page.locator('.session-card').filter({ has: page.getByText(title, { exact: true }) });
  const expand = card('父任务').getByRole('button', { name: '展开 父任务 的 1 个子会话' });
  await expand.waitFor();
  assert.equal(await page.locator('.session-card').count(), 3);
  await page.locator('.sidebar-project-list .project-meta').getByText('5 个会话', { exact: true }).waitFor();
  await page.getByRole('combobox', { name: '当前 Agent' }).click();
  assert.equal(await page.getByRole('option', { name: 'Codex', exact: true }).getAttribute('aria-description'), '5 个会话');
  await page.keyboard.press('Escape');
  assert.equal(await card('孤儿任务').locator('.orphan-ribbon').textContent(), '孤儿');
  assert.equal(await card('独立执行').getByText('exec', { exact: true }).count(), 1);
  await expand.click();
  await card('一级子任务').getByRole('button', { name: '展开 一级子任务 的 1 个子会话' }).click();
  await card('二级子任务').locator('.session-card-open').click();
  const header = page.locator('.detail-header');
  await header.getByRole('button', { name: '打开父会话 child' }).waitFor();
  const sessionId = header.getByRole('button', { name: '复制会话 ID：leaf' });
  const parentId = header.getByRole('button', { name: '复制父会话 ID：child' });
  await parentId.waitFor();
  assert.equal(await parentId.getAttribute('class'), await sessionId.getAttribute('class'));
  assert.equal(await parentId.locator('code').textContent(), 'child');
  assert.equal(await parentId.locator('.copy-id-icon').count(), 1);
  assert.equal(await header.getByRole('button', { name: '在终端恢复' }).count(), 0);
  assert.equal(await header.getByRole('button', { name: '归档', exact: true }).count(), 1);
  const inherited = page.locator('.inherited-head');
  await inherited.getByText('2 条消息', { exact: true }).waitFor();
  await page.getByText('子会话正文样例', { exact: true }).waitFor();
  assert.equal(await page.getByText('父会话的回答', { exact: true }).count(), 0);
  await inherited.click();
  await page.getByText('父会话的回答', { exact: true }).waitFor();
  assert.equal(await page.locator('.event-group.inherited').count(), 2);
  await page.getByText('子会话开始', { exact: true }).waitFor();
  await page.getByRole('button', { name: '收起继承内容' }).click();
  await page.getByText('父会话的回答', { exact: true }).waitFor({ state: 'detached' });
  assert.equal(await inherited.getAttribute('aria-expanded'), 'false');
  await card('父任务').getByRole('button', { name: '收起 父任务 的 1 个子会话' }).click();
  await header.getByRole('button', { name: '在列表定位' }).click();
  await card('二级子任务').locator('.session-card-open').waitFor();
  await card('父任务').hover();
  await card('父任务').getByRole('button', { name: '删除（父任务）' }).click();
  await page.getByRole('dialog').getByRole('button', { name: '永久删除', exact: true }).click();
  await page.locator('.placeholder').waitFor();
  await page.locator('.sidebar-project-list .project-meta').getByText('2 个会话', { exact: true }).waitFor();
  assert.equal(await page.locator('.session-card').count(), 2);
  assert.deepEqual(errors, []);
  console.log(`PASS: nested Codex sessions, inherited fork context, orphans, all-session counts, and cascade deletion (${engine})`);
} finally {
  await browser?.close();
  rmSync(dir, { recursive: true, force: true });
}
