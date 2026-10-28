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
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
      window.__CONTEXT_CALLS__.push({ cmd, args });
      if (cmd === 'list_agents') return [{ id: 'codex', displayName: 'Codex', installed: true, deletePolicy: 'trash' }];
      if (cmd === 'catalog') return structuredClone(args.includeEmpty ? fixture.all : fixture.catalog);
      if (cmd === 'read_session') return structuredClone(fixture.details.find(d => d.meta.sessionId === args.sessionId));
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
  const pageState = page.locator('.collapsible.context').filter({ has: page.getByRole('button', { name: '页面上下文 · 未打开页面', exact: true }) }).first();
  await pageState.getByRole('button').click();
  assert.equal(await pageState.locator('.code').textContent(), '<external_codex_apps_open_page>{"page_id":null}</external_codex_apps_open_page>');
  await page.getByRole('checkbox', { name: 'Markdown', exact: true }).check();
  assert.ok((await pageState.locator('.code').textContent()).includes('<external_codex_apps_open_page>'));
  await pageState.getByRole('button').click();
  assert.equal(await page.locator('.toc-item').count(), 2);
  await page.waitForFunction(() => {
    const rows = [...document.querySelectorAll('.event-group')];
    return rows.every((row, i) => i === 0 || Math.abs(row.getBoundingClientRect().top - rows[i - 1].getBoundingClientRect().bottom - 10) < 1);
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
  await page.getByRole('checkbox', { name: '显示空会话', exact: true }).check();
  await page.waitForFunction(() => document.querySelectorAll('.session-card').length === 2);
  const index = fixture.all.sessions.findIndex(s => s.sessionId === 'context-only');
  await page.locator('.session-card').nth(index).click();
  await page.getByText('暂无对话正文，可勾选“上下文”查看运行记录', { exact: true }).waitFor();
  assert.equal(await page.locator('.toc-item').count(), 0);
  await context.check();
  await page.waitForFunction(() => document.querySelectorAll('.collapsible.context').length === 2);
  await page.setViewportSize({ width: 960, height: 760 });
  assert.ok(await page.locator('.detail-header').evaluate(el => el.scrollWidth <= el.clientWidth + 1));
  assert.deepEqual(errors, []);
  console.log(`PASS: context-only sessions stay empty, retain inspectable records and fit the minimum window width (${engine})`);
} finally {
  await browser?.close();
  rmSync(dir, { recursive: true, force: true });
}
