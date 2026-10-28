import { engine, baseURL } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const dir = mkdtempSync(path.join(tmpdir(), 'asm-empty-path-'));
const exported = path.join(dir, 'catalogs.json');
let browser;
try {
  const run = await promisify(execFile)('cargo', ['test', '--offline', '--locked',
    'empty_sessions_keep_authoritative_project_path_when_toggled', '--', '--nocapture'], {
    cwd: path.resolve('src-tauri'), env: { ...process.env, ASM_EMPTY_PATH_EXPORT: exported }, timeout: 180000,
  });
  assert.match(run.stdout, /1 passed/);
  const fixture = JSON.parse(readFileSync(exported, 'utf8'));
  browser = engine === 'webkit' ? await webkit.launch({ headless: true })
    : await chromium.launch({ channel: 'chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  const errors = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.addInitScript(fixture => {
    window.__PATH_CALLS__ = [];
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (cmd, args = {}) => {
      window.__PATH_CALLS__.push({ cmd, args });
      if (cmd === 'list_agents') return [{ id: 'claude', displayName: 'Claude Code', installed: true, deletePolicy: 'trash' }];
      if (cmd === 'catalog') return structuredClone(args.includeEmpty ? fixture.expanded : fixture.collapsed);
      if (cmd === 'read_session') return { meta: fixture.collapsed.sessions[0], warnings: [],
        events: [{ type: 'user', uuid: 'u1', message: { content: 'Existing conversation' } }] };
      return null;
    } };
  }, fixture);
  await page.goto(baseURL);
  const project = page.locator('.sidebar-project-list .project-item');
  await project.waitFor();
  assert.equal(await project.getAttribute('title'), '/Users/demo/.codex');
  await project.click();
  for (const count of [4, 1, 4, 1]) {
    await page.getByRole('checkbox', { name: '显示空会话', exact: true }).setChecked(count === 4);
    await page.waitForFunction(count => document.querySelectorAll('.session-card').length === count, count);
    assert.equal(await project.getAttribute('title'), '/Users/demo/.codex');
    assert.equal(await project.locator('.project-path').textContent(), '~/.codex');
    assert.match(await project.getAttribute('class'), /active/);
    assert.ok((await project.textContent()).includes(`${count} 个会话`));
  }
  await page.locator('.session-card').click();
  await page.locator('.detail-body').getByText('Existing conversation', { exact: true }).waitFor();
  const read = await page.evaluate(() => window.__PATH_CALLS__.find(c => c.cmd === 'read_session'));
  assert.equal(read.args.projectDir, '-Users-demo--codex');
  assert.deepEqual(errors, []);
  console.log(`PASS: empty-session toggles preserve hidden project path, selection, counts and source identity (${engine})`);
} finally {
  await browser?.close();
  rmSync(dir, { recursive: true, force: true });
}
