import { engine, baseURL } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';

const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage();
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    const state = window.__DIRECTORY__ = { opened: [], failure: '' };
    const agents = [
      ['qoder', 'Qoder CLI', '/Users/demo/.qoder'],
      ['claude', 'Claude Code', '/Volumes/Workspace/Claude 数据'],
      ['codex', 'Codex', '/Users/demo/.codex'],
      ['opencode', 'OpenCode', '/Users/demo/.local/share/opencode'],
      ['pi', 'Pi', '/Users/demo/.pi/agent'],
    ];
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__ = { transformCallback: callback => callback, invoke: async (command, args) => {
      if (command === 'list_agents') return agents.map(([id, displayName, dataDir]) => ({ id, displayName, dataDir, installed: true, supportsFork: false, supportsModelNames: false, deletePolicy: 'trash' }));
      if (command === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (command === 'open_data_dir') {
        state.opened.push(args);
        if (state.failure) throw state.failure;
        return null;
      }
      if (command === 'plugin:opener|open_path') throw Error('The frontend must not send an arbitrary path to opener');
      return null;
    } };
  });
  await page.goto(baseURL);
  for (const [id, name] of [['qoder', 'Qoder CLI'], ['claude', 'Claude Code'], ['codex', 'Codex'], ['opencode', 'OpenCode'], ['pi', 'Pi']]) {
    await page.getByRole('combobox', { name: '当前 Agent', exact: true }).selectOption(id);
    const open = page.getByRole('button', { name: '打开数据目录', exact: true });
    assert.equal(await open.getAttribute('title'), `在访达中打开 ${name} 数据目录`);
    await open.click();
    await page.waitForFunction(id => window.__DIRECTORY__.opened.at(-1)?.agent === id, id);
    assert.deepEqual(await page.evaluate(() => window.__DIRECTORY__.opened.at(-1)), { agent: id });
  }
  assert.equal(await page.locator('.toast.error').count(), 0);
  await page.evaluate(() => { window.__DIRECTORY__.failure = '数据目录不存在：/Volumes/Workspace/Claude 数据'; });
  await page.getByRole('button', { name: '打开数据目录', exact: true }).click();
  await page.getByText('打开数据目录失败: 数据目录不存在：/Volumes/Workspace/Claude 数据', { exact: true }).waitFor();
  assert.deepEqual(errors, []);
  console.log(`PASS: all Agent directory buttons route by identity, accept hidden/custom display paths, and report failures (${engine})`);
} finally { await browser.close(); }
