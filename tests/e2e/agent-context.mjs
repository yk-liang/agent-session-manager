import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const dir = mkdtempSync(path.join(tmpdir(), 'asm-agent-context-'));
let browser;
try {
  const result = await promisify(execFile)('cargo', ['test', '--offline', '--locked',
    'context_visibility_', '--', '--test-threads=1'], {
    cwd: path.resolve('src-tauri'), env: { ...process.env, ASM_AGENT_CONTEXT_EXPORT: dir }, timeout: 180000,
  });
  assert.match(result.stdout, /6 passed/);
  const fixtures = readdirSync(dir).map(name => [name, path.join(dir, name)]);
  if (process.env.ASM_AGENT_CONTEXT_LIVE_EXPORT) {
    const live = process.env.ASM_AGENT_CONTEXT_LIVE_EXPORT;
    fixtures.push(...readdirSync(live).filter(n => n.endsWith('.json')).map(n => [n, path.join(live, n)]));
  }
  browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
  for (const [name, file] of fixtures) {
    const fixture = JSON.parse(readFileSync(file, 'utf8'));
    const agent = name.split(/[.-]/)[0];
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    await page.addInitScript(({ fixture, agent }) => {
      window.__CONTEXT_CALLS__ = [];
      window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
        window.__CONTEXT_CALLS__.push({ cmd, args });
        if (cmd === 'list_agents') return [{ id: agent, displayName: agent, installed: true, deletePolicy: 'trash' }];
        if (cmd === 'catalog') return structuredClone(fixture.catalog);
        if (cmd === 'read_session') return structuredClone(fixture.detail);
        if (cmd === 'search_conversations') return args.query === fixture.visible ? fixture.search : { hits: [], skipped: 0, cancelled: false };
        return null;
      } };
    }, { fixture, agent });
    await page.goto(baseURL);
    await page.locator('.session-card').click();
    const toggle = page.getByRole('checkbox', { name: '上下文', exact: true });
    await toggle.waitFor();
    assert.equal(await toggle.isChecked(), false);
    assert.equal(await page.locator('.collapsible.context').count(), 0);
    assert.equal(await page.locator('.toc-item').count(), fixture.detail.meta.realUserCount > 1 ? fixture.detail.meta.realUserCount : 0);
    assert.ok(!(await page.locator('.detail-body').textContent()).includes(fixture.hidden));
    if (name === 'claude.json' || name === 'qoder.json') {
      assert.ok((await page.locator('.detail-body').textContent()).includes('literal instructions'));
      assert.ok((await page.locator('.system-line').textContent()).includes('operational error'));
    }
    await toggle.check();
    const context = page.locator('.collapsible.context').first();
    await context.waitFor();
    assert.equal(await context.getByRole('button').getAttribute('aria-expanded'), 'false');
    await context.getByRole('button').click();
    await context.locator('.code').waitFor();
    assert.ok((await context.locator('.code').textContent()).length > 0);
    await page.getByRole('checkbox', { name: 'Markdown', exact: true }).check();
    assert.ok(await context.locator('.code').count());
    await context.getByRole('button').click();
    if (name === 'claude.json' || name === 'pi.json') {
      await page.waitForFunction(() => {
        const rows = [...document.querySelectorAll('.event-group')];
        return rows.every((row, i) => i === 0 || Math.abs(row.getBoundingClientRect().top - rows[i - 1].getBoundingClientRect().bottom - 10) < 1);
      });
      await page.screenshot({ path: path.join(artifactDir, `agent-context-${agent}-${engine}.png`) });
    }
    await toggle.uncheck();
    await page.getByRole('searchbox').fill(fixture.visible);
    await page.locator('.session-search-snippet').waitFor();
    await page.locator('.session-card').click();
    await page.locator('.search-target mark').first().waitFor();
    assert.equal(await page.locator('.collapsible.context').count(), 0);
    await page.getByRole('searchbox').fill(fixture.hidden);
    await page.waitForFunction(query => window.__CONTEXT_CALLS__.some(c => c.cmd === 'search_conversations' && c.args.query === query)
      && document.querySelectorAll('.session-card').length === 0, fixture.hidden);
    await page.setViewportSize({ width: 960, height: 760 });
    assert.ok(await page.locator('.detail-header').evaluate(el => el.scrollWidth <= el.clientWidth + 1));
    assert.deepEqual(errors, []);
    console.log(`PASS: ${name}: default-hidden context, raw expandable records, dialogue counts, literal markup and search positioning (${engine})`);
    await page.close();
  }
} finally { await browser?.close(); rmSync(dir, { recursive: true, force: true }); }
