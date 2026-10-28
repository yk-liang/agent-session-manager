import { engine, baseURL, artifactDir } from '../helpers/browser.mjs';
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import path from 'node:path';
import { configFixtures, configMockBootstrap } from '../fixtures/config.mjs';

const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    const names = { qoder: 'Qoder CLI', claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', pi: 'Pi' };
    const calls = window.__NAVIGATION__ = [];
    window.__TAURI_INTERNALS__ = { transformCallback: callback => callback, invoke: async (command, args = {}) => {
      calls.push({ command, args });
      if (command === 'list_agents') return Object.entries(names).map(([id, displayName]) => ({ id, displayName, installed: !location.search.includes('empty') && id !== 'pi', supportsFork: false, dataDir: `/Users/demo/.${id}`, deletePolicy: 'trash' }));
      if (command === 'catalog') return { sessions: [], projects: [], warnings: [] };
      if (command === 'collector_status') return { desired: false, running: false, authorization: 'disabled' };
      if (command === 'sync_request_events') return { updatedAt: Date.now(), checkedFiles: 0, changedFiles: 0, bytesRead: 0 };
      if (command === 'read_usage_summary') return { rows: [], health: {}, revision: '1' };
      if (command === 'read_request_events') return { events: [], models: [], total: 0, nextCursor: null, reset: false, revision: '1' };
      return null;
    } };
  });
  await page.addInitScript(configMockBootstrap, configFixtures);
  await page.goto(baseURL);
  const switcher = page.getByRole('combobox', { name: '当前 Agent', exact: true });
  const tab = name => page.getByRole('button', { name, exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.agent-switcher select')?.value === 'qoder');
  assert.equal(await switcher.locator('option[value="all"]').count(), 0);
  assert.ok(await switcher.locator('option[value="pi"]').isDisabled());
  await switcher.selectOption('claude');
  await tab('配置');
  await page.getByText('claude-demo', { exact: true }).waitFor();
  assert.equal(await switcher.inputValue(), 'claude');
  assert.equal(await page.locator('.sidebar').count(), 0);

  await page.getByRole('button', { name: '编辑', exact: true }).click();
  await page.locator('textarea').fill('{"model":"unsaved"}');
  await switcher.selectOption('codex');
  await page.getByRole('dialog', { name: '放弃未保存修改？' }).waitFor();
  assert.equal(await switcher.inputValue(), 'claude', 'pending navigation must keep the current Agent visible');
  await page.getByRole('dialog').getByRole('button', { name: '取消', exact: true }).click();
  assert.equal(await page.locator('textarea').inputValue(), '{"model":"unsaved"}');
  await page.getByRole('button', { name: '取消编辑', exact: true }).click();

  await tab('用量');
  assert.equal(await switcher.inputValue(), 'claude');
  await switcher.selectOption('all');
  await tab('配置');
  await page.getByText('claude-demo', { exact: true }).waitFor();
  assert.equal(await switcher.inputValue(), 'claude');
  await tab('用量');
  assert.equal(await switcher.inputValue(), 'all', 'the aggregate usage scope survives a visit to configuration');
  await switcher.selectOption('codex');
  await tab('会话');
  assert.equal(await switcher.inputValue(), 'codex');
  await page.waitForFunction(() => window.__NAVIGATION__.some(call => call.command === 'catalog' && call.args.agent === 'codex'));
  await tab('配置');
  await page.getByText('codex-demo', { exact: true }).waitFor();
  assert.equal(await switcher.inputValue(), 'codex');
  console.log(`PASS: one Agent selection follows all three pages; all-Agent usage retains the local context; cancelled navigation preserves drafts (${engine})`);

  await tab('用量');
  await switcher.selectOption('pi');
  await page.waitForFunction(() => window.__NAVIGATION__.some(call => call.command === 'read_request_events' && call.args.query.agent === 'pi'));
  await tab('配置');
  await page.getByText('codex-demo', { exact: true }).waitFor();
  assert.equal(await switcher.inputValue(), 'codex', 'historical-only sources cannot become configuration targets');
  assert.ok(await switcher.locator('option[value="pi"]').isDisabled());
  assert.equal(await page.locator('.sidebar,.pane-divider').count(), 0);

  for (const width of [960, 1280, 1600]) {
    await page.setViewportSize({ width, height: 900 });
    let position;
    for (const name of ['会话', '配置', '用量']) {
      await tab(name);
      const box = await switcher.boundingBox();
      if (position) assert.deepEqual(box, position, `switcher moved in ${name} at ${width}`);
      position = box;
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
      const layout = await page.locator('.topbar').evaluate(el => [...el.children].map(child => {
        const box = child.getBoundingClientRect(); return { left: box.left, right: box.right };
      }));
      assert.ok(layout.every((box, index) => box.right <= width && (!index || box.left >= layout[index - 1].right)), `toolbar overlaps at ${width}`);
    }
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await switcher.selectOption('claude');
  for (const [name, file] of [['会话', 'sessions'], ['配置', 'config'], ['用量', 'usage']]) {
    await tab(name);
    if (name === '配置') await page.getByText('claude-demo', { exact: true }).waitFor();
    await page.screenshot({ path: path.join(artifactDir, `agent-navigation-${file}.png`) });
  }
  await page.goto(`${baseURL}?empty=1`);
  await page.waitForFunction(() => document.querySelector('.agent-switcher select')?.value === '');
  assert.equal(await page.getByRole('button', { name: '配置', exact: true }).count(), 0);
  await tab('用量');
  await switcher.selectOption('pi');
  await tab('会话');
  assert.equal(await switcher.inputValue(), '');
  assert.deepEqual(errors, []);
  console.log(`PASS: historical-only and empty installs remain safe; the control stays fixed and fits 960/1280/1600 layouts (${engine})`);
} finally { await browser.close(); }
