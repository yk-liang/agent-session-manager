import { chooseAgent } from '../helpers/agent-switcher.mjs';
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
  await page.waitForFunction(() => document.querySelector('.agent-switcher-trigger')?.value === 'qoder');
  await switcher.click();
  assert.equal(await page.locator('[role="option"][value="all"]').count(), 0);
  assert.ok(await page.locator('[role="option"][value="pi"]').isDisabled());
  await page.keyboard.press('Escape');
  await chooseAgent(page, 'claude');
  await tab('配置');
  await page.getByText('claude-demo', { exact: true }).waitFor();
  assert.equal(await switcher.getAttribute("value"), 'claude');
  assert.equal(await page.locator('.sidebar').count(), 0);

  await page.getByRole('button', { name: '编辑', exact: true }).click();
  await page.locator('textarea').fill('{"model":"unsaved"}');
  await chooseAgent(page, 'codex');
  await page.getByRole('dialog', { name: '放弃未保存修改？' }).waitFor();
  assert.equal(await switcher.getAttribute("value"), 'claude', 'pending navigation must keep the current Agent visible');
  await page.getByRole('dialog').getByRole('button', { name: '取消', exact: true }).click();
  assert.equal(await page.locator('textarea').inputValue(), '{"model":"unsaved"}');
  await page.getByRole('button', { name: '取消编辑', exact: true }).click();

  await tab('用量');
  assert.equal(await switcher.getAttribute("value"), 'claude');
  await chooseAgent(page, 'all');
  await tab('配置');
  await page.getByText('claude-demo', { exact: true }).waitFor();
  assert.equal(await switcher.getAttribute("value"), 'claude');
  await tab('用量');
  assert.equal(await switcher.getAttribute("value"), 'all', 'the aggregate usage scope survives a visit to configuration');
  await chooseAgent(page, 'codex');
  await tab('会话');
  assert.equal(await switcher.getAttribute("value"), 'codex');
  await page.waitForFunction(() => window.__NAVIGATION__.some(call => call.command === 'catalog' && call.args.agent === 'codex'));
  await tab('配置');
  await page.getByText('codex-demo', { exact: true }).waitFor();
  assert.equal(await switcher.getAttribute("value"), 'codex');
  console.log(`PASS: one Agent selection follows all three pages; all-Agent usage retains the local context; cancelled navigation preserves drafts (${engine})`);

  await tab('用量');
  await chooseAgent(page, 'pi');
  await page.waitForFunction(() => window.__NAVIGATION__.some(call => call.command === 'read_request_events' && call.args.query.agent === 'pi'));
  await tab('配置');
  await page.getByText('codex-demo', { exact: true }).waitFor();
  assert.equal(await switcher.getAttribute("value"), 'codex', 'historical-only sources cannot become configuration targets');
  await switcher.click();
  assert.ok(await page.locator('[role="option"][value="pi"]').isDisabled());
  await page.keyboard.press('Escape');
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

  const menu = page.getByRole('listbox', { name: 'Agent', exact: true });
  await tab('用量');
  for (const viewport of [{ width: 960, height: 600 }, { width: 1280, height: 800 }]) {
    await page.setViewportSize(viewport);
    for (const id of ['codex', 'opencode', 'pi']) {
      await chooseAgent(page, id);
      await switcher.click();
      const buttonBox = await switcher.boundingBox(), menuBox = await menu.boundingBox();
      assert.ok(menuBox.y >= buttonBox.y + buttonBox.height + 4, `menu must open below ${id}`);
      assert.ok(menuBox.x >= 0 && menuBox.x + menuBox.width <= viewport.width);
      assert.ok(menuBox.y + menuBox.height <= viewport.height);
      assert.equal(await menu.getByRole('option').count(), 6);
      assert.equal(await menu.locator(`[value="${id}"]`).getAttribute('aria-selected'), 'true');
      for (const option of await menu.getByRole('option').all()) {
        const box = await option.boundingBox();
        assert.ok(box.y >= menuBox.y && box.y + box.height <= menuBox.y + menuBox.height, 'every option must be visible, including earlier Agents');
      }
      if (id === 'pi') await page.screenshot({ path: path.join(artifactDir, `agent-switcher-open-pi-${viewport.width}.png`) });
      await page.keyboard.press('Escape');
      assert.equal(await switcher.getAttribute('aria-expanded'), 'false');
    }
  }
  console.log(`PASS: reopening Codex, OpenCode and Pi shows the complete menu below its trigger at 960x600 and 1280x800 (${engine})`);

  await chooseAgent(page, 'codex');
  await switcher.press('ArrowDown');
  assert.ok((await switcher.getAttribute('aria-activedescendant')).endsWith('-codex'));
  assert.ok(await switcher.evaluate(el => el === document.activeElement));
  await switcher.press('End');
  assert.ok((await switcher.getAttribute('aria-activedescendant')).endsWith('-pi'));
  assert.equal(await switcher.getAttribute('value'), 'codex', 'keyboard preview must not switch the Agent');
  await switcher.press('Escape');
  assert.equal(await switcher.getAttribute('value'), 'codex');
  await switcher.press('Home');
  await switcher.press('ArrowDown');
  await switcher.press('Enter');
  assert.equal(await switcher.getAttribute('value'), 'qoder');
  assert.equal(await menu.count(), 0);
  await switcher.press('Space');
  await switcher.press('End');
  await switcher.press('Space');
  assert.equal(await switcher.getAttribute('value'), 'pi');
  await switcher.press('Home');
  await switcher.press('Tab');
  assert.equal(await menu.count(), 0);
  assert.equal(await switcher.getAttribute('value'), 'pi', 'Tab leaves the picker without an accidental Agent switch');
  assert.ok(!await switcher.evaluate(el => el === document.activeElement));
  await switcher.press('c');
  assert.ok((await switcher.getAttribute('aria-activedescendant')).endsWith('-claude'));
  await switcher.press('c');
  assert.ok((await switcher.getAttribute('aria-activedescendant')).endsWith('-codex'));
  await switcher.press('Escape');
  await switcher.press('c');
  await switcher.press('o');
  assert.ok((await switcher.getAttribute('aria-activedescendant')).endsWith('-codex'));
  await switcher.press('Enter');
  assert.equal(await switcher.getAttribute('value'), 'codex');
  await switcher.click();
  await switcher.press('End');
  await page.locator('.topbar').click({ position: { x: 2, y: 2 } });
  assert.equal(await menu.count(), 0);
  assert.equal(await switcher.getAttribute('value'), 'codex');
  await switcher.click();
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  assert.equal(await menu.count(), 0);
  await tab('配置');
  await switcher.press('End');
  assert.ok((await switcher.getAttribute('aria-activedescendant')).endsWith('-opencode'), 'keyboard navigation skips an unavailable Agent');
  await switcher.press('Escape');
  console.log(`PASS: keyboard preview/commit, typeahead, disabled options, Escape, Tab, outside click and window blur preserve focus and selection (${engine})`);

  await page.setViewportSize({ width: 1280, height: 900 });
  await chooseAgent(page, 'claude');
  for (const [name, file] of [['会话', 'sessions'], ['配置', 'config'], ['用量', 'usage']]) {
    await tab(name);
    if (name === '配置') await page.getByText('claude-demo', { exact: true }).waitFor();
    await page.screenshot({ path: path.join(artifactDir, `agent-navigation-${file}.png`) });
  }
  await page.goto(`${baseURL}?empty=1`);
  await page.waitForFunction(() => document.querySelector('.agent-switcher-trigger')?.value === '');
  assert.ok(await switcher.isDisabled());
  assert.equal(await page.getByRole('button', { name: '配置', exact: true }).count(), 0);
  await tab('用量');
  await chooseAgent(page, 'pi');
  await tab('会话');
  assert.equal(await switcher.getAttribute("value"), '');
  assert.deepEqual(errors, []);
  console.log(`PASS: historical-only and empty installs remain safe; the control stays fixed and fits 960/1280/1600 layouts (${engine})`);
} finally { await browser.close(); }
