import assert from 'node:assert/strict';
import { chromium, webkit } from 'playwright';
import { baseURL, engine } from '../helpers/browser.mjs';
import { chooseAgent } from '../helpers/agent-switcher.mjs';
import { chooseOption } from '../helpers/dropdown.mjs';
import { installScreenshotFixture, screenshotFixture, screenshotTime } from '../../scripts/readme-fixture.mjs';

const browser = engine === 'webkit' ? await webkit.launch() : await chromium.launch({ channel: 'chrome' });
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, timezoneId: 'America/Los_Angeles' });
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on('pageerror', error => { errors.push(error.message); console.error(error); });
  await page.clock.setFixedTime(new Date(screenshotTime));
  await page.addInitScript({ content: `(${(function init(fixture, install) {
    const codex = fixture.agents.find(agent => agent.id === 'codex');
    codex.supportsArchivedSessions = true;
    codex.installed = !location.search.includes('no-codex');
    const first = fixture.catalog.sessions[0];
    const sessions = [...fixture.catalog.sessions,
      { ...first, sessionId: 'empty-fixture', title: '空会话样例', realUserCount: 0 },
      { ...first, sessionId: 'archived-fixture', title: '归档会话样例', archived: true }];
    install(fixture);
    const original = window.__TAURI_INTERNALS__.invoke;
    const calls = window.__PREFS_CALLS__ = [];
    window.__TAURI_INTERNALS__.invoke = async (cmd, args = {}) => {
      calls.push({ cmd, args });
      if (cmd === 'catalog') return { ...fixture.catalog, sessions: sessions.filter(session =>
        (args.includeEmpty || session.realUserCount) && (args.includeArchived || !session.archived)) };
      return original(cmd, args);
    };
  })})(${JSON.stringify(screenshotFixture)}, ${installScreenshotFixture});` });

  const tab = name => page.getByRole('button', { name, exact: true });
  const usageTab = name => page.getByRole('tab', { name, exact: true });
  const checkbox = name => page.getByRole('checkbox', { name, exact: true });
  const pick = (group, name) => page.getByRole('group', { name: group, exact: true }).getByRole('button', { name, exact: true });
  const combo = name => page.getByRole('combobox', { name, exact: true });
  const card = name => page.locator('.session-card').filter({ hasText: name });
  const firstCall = command => page.evaluate(command => window.__PREFS_CALLS__.find(call => call.cmd === command)?.args, command);

  await page.goto(baseURL);
  await chooseAgent(page, 'codex');
  await checkbox('空会话').check();
  await card('空会话样例').waitFor();
  await checkbox('已归档').check();
  await card('归档会话样例').waitFor();
  await card('给笔记应用添加全文搜索').click();
  await checkbox('工具调用').uncheck();
  await checkbox('思考过程').uncheck();
  await checkbox('上下文').check();
  // Legacy Markdown is enabled by the fixture; an explicit new false must override it.
  await checkbox('Markdown').uncheck();
  await chooseAgent(page, 'qoder');
  await checkbox('空会话').uncheck();
  assert.equal(await checkbox('已归档').count(), 0);
  await checkbox('空会话').check();
  await chooseAgent(page, 'codex');
  await card('归档会话样例').waitFor();
  await page.getByRole('searchbox').fill('临时搜索');
  await page.reload();
  await card('归档会话样例').waitFor({ timeout: 3000 });
  assert.equal(await combo('当前 Agent').getAttribute('value'), 'codex');
  assert.equal(await checkbox('空会话').isChecked(), true);
  assert.equal(await checkbox('已归档').isChecked(), true);
  assert.equal(await page.getByRole('searchbox').inputValue(), '');
  const catalog = await firstCall('catalog');
  assert.deepEqual([catalog.agent, catalog.includeEmpty, catalog.includeArchived], ['codex', true, true]);
  await card('给笔记应用添加全文搜索').click();
  await checkbox('上下文').waitFor();
  for (const [name, checked] of [['工具调用', false], ['思考过程', false], ['上下文', true], ['Markdown', false]]) {
    assert.equal(await checkbox(name).isChecked(), checked, name);
  }
  console.log(`PASS: session controls restore on first scan/reload and across unsupported Agents (${engine})`);

  await tab('用量').click();
  await chooseAgent(page, 'all');
  const analysis = () => usageTab('统计分析').click();
  const events = () => usageTab('请求记录').click();
  const prices = () => usageTab('模型价格').click();
  const checkedChoice = async (control, name) => {
    assert.equal(await pick(control, name).getAttribute('aria-pressed'), 'true');
  };
  await pick('时间范围', '近 7 天').click();
  await pick('趋势指标', '请求').click();
  await pick('趋势分组', '按 Agent').click();
  await pick('分组维度', '按提供商').click();
  await pick('占比指标', '费用占比').click();
  await events();
  await chooseOption(page, '模型', 'value:gpt-5-codex');
  await chooseOption(page, '项目', 'value:/Users/demo/Projects/atlas-notes');
  await chooseOption(page, '请求结果', 'value:success');
  await chooseOption(page, '每页请求数', '100');
  await chooseAgent(page, 'codex');
  assert.equal(await combo('模型').getAttribute('value'), 'all');
  assert.equal(await combo('项目').getAttribute('value'), 'all');
  await chooseOption(page, '请求结果', 'value:failed');
  await analysis();
  await checkedChoice('趋势分组', '按类别');
  await chooseAgent(page, 'all');
  await checkedChoice('趋势分组', '按 Agent');
  await checkedChoice('趋势指标', '请求');
  await checkedChoice('分组维度', '按提供商');
  await checkedChoice('占比指标', '费用占比');
  await events();
  const assertAllFilters = async () => {
    assert.equal(await combo('模型').getAttribute('value'), 'value:gpt-5-codex');
    assert.equal(await combo('项目').getAttribute('value'), 'value:/Users/demo/Projects/atlas-notes');
    assert.equal(await combo('请求结果').getAttribute('value'), 'value:success');
    assert.equal(await combo('每页请求数').getAttribute('value'), '100');
  };
  await assertAllFilters();
  await tab('会话').click();
  await tab('用量').click();
  assert.equal(await usageTab('请求记录').getAttribute('aria-selected'), 'true');
  await checkedChoice('时间范围', '近 7 天');
  await assertAllFilters();

  await page.clock.setFixedTime(new Date(Date.parse(screenshotTime) + 24 * 60 * 60_000));
  await page.reload();
  assert.equal(await tab('会话').getAttribute('aria-current'), 'page');
  await tab('用量').click();
  await assertAllFilters();
  const query = (await firstCall('read_request_events')).query;
  assert.deepEqual([query.agent, query.limit, query.model, query.project, query.status, query.page],
    [undefined, 100, 'gpt-5-codex', '/Users/demo/Projects/atlas-notes', 'success', 1]);
  const { from, bounds } = (await firstCall('read_usage_summary')).query;
  assert.equal(bounds.length, 8);
  assert.equal(from, Date.parse('2026-10-28T00:00:00-07:00'));
  await chooseAgent(page, 'codex');
  assert.equal(await combo('请求结果').getAttribute('value'), 'value:failed');
  await prices();
  const custom = page.getByRole('group', { name: '定价状态筛选' }).getByRole('button', { name: /^自定义/ });
  await custom.click();
  await page.getByRole('searchbox', { name: '搜索模型价格', exact: true }).fill('临时关键字');
  await page.reload();
  await tab('用量').click();
  await custom.waitFor();
  assert.equal(await usageTab('模型价格').getAttribute('aria-selected'), 'true');
  assert.equal(await custom.getAttribute('aria-pressed'), 'true');
  assert.equal(await page.getByRole('searchbox', { name: '搜索模型价格', exact: true }).inputValue(), '');
  await page.waitForFunction(() => window.__PREFS_CALLS__.some(call => call.cmd === 'get_model_prices'));
  assert.equal((await firstCall('get_model_prices')).agent, 'codex');
  await analysis();
  await chooseAgent(page, 'all');
  for (const [group, name] of [['趋势指标', '请求'], ['趋势分组', '按 Agent'], ['分组维度', '按提供商'], ['占比指标', '费用占比']]) {
    await checkedChoice(group, name);
  }
  await chooseAgent(page, 'codex');
  await prices();
  await page.goto(new URL('?no-codex', baseURL).href);
  await page.waitForFunction(() => document.querySelector('.agent-switcher .dropdown-trigger')?.value === 'qoder');
  await page.waitForFunction(() => window.__PREFS_CALLS__.some(call => call.cmd === 'catalog'));
  assert.equal((await firstCall('catalog')).agent, 'qoder');
  await tab('用量').click();
  assert.equal(await combo('当前 Agent').getAttribute('value'), 'all');
  await page.waitForFunction(() => window.__PREFS_CALLS__.some(call => call.cmd === 'get_model_prices'));
  assert.equal((await firstCall('get_model_prices')).agent, null);
  assert.deepEqual(errors, []);
  console.log(`PASS: usage restores subpage, period, chart controls, scoped filters, page size and model prices (${engine})`);
} finally {
  await browser.close();
}
