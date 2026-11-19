import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { engine, baseURL } from '../helpers/browser.mjs';
import { chooseOption, openDropdown } from '../helpers/dropdown.mjs';

const opus = 'claude-opus-5-5';
const gemini = 'gemini-4-neon';
const geminiAlias = 'Gemini 4 Neon (XHigh)';
const deepseek = 'deepseek-cqu/DeepSeek-V4.1-Flash-1017';
const longModel = `${deepseek}-${'context-'.repeat(5)}preview`;
const oversizedModel = `openai/${'gpt-test-'.repeat(25)}preview`;
const extraModels = [deepseek, longModel, oversizedModel, 'deepseek-flash', 'openai/gpt-6.1-terra',
  'aliyun/deepseek-v4.1-flash-1017', 'claude-fable-5-2', 'gemini-4-argon', 'google-vertex/gemini-4-argon'];
const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(({ opus, gemini, geminiAlias, extraModels }) => {
    const agent = 'antigravity';
    const models = [[opus, 'Claude Opus 5.5'], [gemini, geminiAlias], ['unknown', 'Unknown model'],
      ...extraModels.map(model => [model, model])];
    const session = { sessionId: 'fixture-session', projectDir: 'fixture', cwd: '/tmp/fixture', title: 'Model fixture',
      model: gemini, modelName: geminiAlias, realUserCount: 1, assistantTextCount: 1, sizeBytes: 2000, hasTranscript: true };
    const tokens = { inputTokens: 100, outputTokens: 10, cacheReadTokens: 80, cacheWriteTokens: 0, reasoningTokens: 0,
      unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true, cacheReadRecorded: true };
    const today = new Date().setHours(12, 0, 0, 0);
    const rows = models.map(([model, modelName]) => ({ ...tokens, agent, model, modelName, bucketStart: today,
      date: '', totalTokens: 110, recordCount: 1, requestCount: model === gemini ? 0 : 1,
      requestCountIncomplete: model === gemini, partial: false }));
    const events = models.map(([model, modelName], index) => ({ ...tokens, id: `request-${index}`, agent, model, modelName,
      time: today - (index + 1) * 1000, session: session.sessionId, status: 'success', partial: false, request: {} }));
    const yesterday = new Date(today).setDate(new Date(today).getDate() - 1);
    rows.push({ ...rows[0], agent: 'codex', model: gemini, bucketStart: yesterday });
    events.push({ ...events[0], agent: 'codex', model: gemini, id: 'older-request', time: yesterday });
    const inRange = (time, query) => (query.from == null || time >= query.from) && (query.to == null || time < query.to);
    const queries = window.__MODEL_QUERIES__ = [];
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (command, args = {}) => {
      if (command === 'list_agents') return [{ id: agent, displayName: 'Antigravity', installed: true, deletePolicy: 'readOnly' },
        { id: 'codex', displayName: 'Codex', installed: true, deletePolicy: 'trash' }];
      if (command === 'catalog') return { sessions: [session], warnings: [], projects: [
        { dirName: session.projectDir, displayPath: session.cwd, sessionCount: 1, lastActivity: null },
      ] };
      if (command === 'read_session') return { meta: session, events: [{ type: 'user', message: { content: 'Model fixture' } }] };
      if (command === 'search_conversations') return { hits: [], skipped: 0, cancelled: false };
      if (command === 'read_usage_summary') return { rows: rows.filter(row => inRange(row.bucketStart, args.query)),
        syncedAgents: [agent, 'codex'], revision: '1' };
      if (command === 'read_request_events') {
        queries.push(args.query.model);
        const scope = events.filter(event => inRange(event.time, args.query) && (!args.query.agent || event.agent === args.query.agent));
        const page = scope.filter(event => !args.query.model || event.model === args.query.model);
        const models = Object.fromEntries(scope.map(event => [event.model, scope.filter(entry => entry.model === event.model).length]));
        return { events: page, total: page.length, page: 1, pageSize: 50, revision: '1', filters: { models, projects: {}, results: {} } };
      }
      return null;
    }};
  }, { opus, gemini, geminiAlias, extraModels });

  await page.goto(baseURL);
  const card = page.locator('.session-card');
  const badge = card.locator('.model-label');
  await badge.waitFor();
  assert.equal(await badge.textContent(), gemini);
  assert.equal(await badge.getAttribute('title'), gemini);
  await card.click();
  await page.getByRole('button', { name: '更多信息', exact: true }).click();
  assert.equal(await page.locator('.session-info-popover .model-label').textContent(), gemini);
  await page.getByRole('searchbox').fill(geminiAlias);
  await page.locator('.session-cards .empty-hint').waitFor();
  await page.getByRole('searchbox').fill(gemini);
  await badge.waitFor();

  await page.getByRole('button', { name: '用量', exact: true }).click();
  const summaryLabels = page.locator('.usage-models tbody .model-label');
  await summaryLabels.first().waitFor();
  const labels = [opus, gemini, '未知模型', ...extraModels].sort();
  const count = (list, name) => list.getByRole('option', { name, exact: true }).locator('.dropdown-count');
  const picker = await openDropdown(page, '当前 Agent');
  await count(picker, '全部 Agent').getByText(String(labels.length + 1), { exact: true }).waitFor();
  assert.equal(await count(picker, 'Antigravity').textContent(), String(labels.length));
  assert.equal(await count(picker, 'Codex').textContent(), '1');
  assert.equal(await page.locator('.agent-switcher .dropdown-trigger .dropdown-count').count(), 0);
  await picker.getByRole('option', { name: 'Antigravity', exact: true }).click();
  const showModels = page.getByRole('button', { name: `显示全部 ${labels.length} 项`, exact: true });
  await showModels.click();
  assert.deepEqual((await summaryLabels.allTextContents()).sort(), labels);
  assert.equal(await page.getByRole('button', { name: '收起', exact: true }).getAttribute('aria-expanded'), 'true');
  await page.getByRole('tab', { name: '请求记录', exact: true }).click();
  const requestLabels = page.locator('.request-model .model-label');
  await page.waitForFunction(length => document.querySelectorAll('.request-model .model-label').length === length, labels.length);
  assert.deepEqual((await requestLabels.allTextContents()).sort(), labels);
  await page.setViewportSize({ width: 960, height: 760 });
  const list = await openDropdown(page, '模型');
  assert.deepEqual((await list.locator('.dropdown-option-label').allTextContents()).sort(), [...labels, '全部模型'].sort());
  assert.equal(await list.getByRole('option', { name: '全部模型', exact: true }).locator('.dropdown-count').textContent(),
    String(labels.length));
  assert.equal(await list.getByRole('option', { name: gemini, exact: true }).locator('.dropdown-count').textContent(), '1');
  const layout = await list.evaluate((element, ids) => {
    const menu = element.parentElement.getBoundingClientRect();
    const row = id => {
      const option = [...element.querySelectorAll('[role="option"]')].find(button => button.getAttribute('aria-label') === id);
      const label = option.querySelector('.dropdown-option-label');
      const text = document.createRange();
      text.selectNodeContents(label);
      const lines = new Set([...text.getClientRects()].map(rect => Math.round(rect.top))).size;
      return { height: option.getBoundingClientRect().height, lines,
        truncated: text.getBoundingClientRect().width > label.getBoundingClientRect().width + 1 };
    };
    return { viewport: innerWidth, left: menu.left, right: menu.right, scrolls: element.scrollHeight > element.clientHeight,
      long: row(ids.longModel), oversized: row(ids.oversizedModel), short: row(ids.deepseek) };
  }, { deepseek, longModel, oversizedModel });
  assert.ok(layout.left >= 7 && layout.right <= layout.viewport - 7);
  assert.ok(layout.scrolls);
  assert.deepEqual(layout.long, { ...layout.short, lines: 1, truncated: false });
  assert.deepEqual(layout.oversized, { ...layout.short, lines: 1, truncated: true });

  const search = page.getByRole('combobox', { name: '搜索模型', exact: true });
  await search.fill(deepseek);
  const wideLabel = list.getByRole('option', { name: longModel, exact: true }).locator('.dropdown-option-label');
  const labelWidth = await wideLabel.evaluate(label => ({
    available: label.clientWidth, content: label.scrollWidth,
    gutter: label.closest('.dropdown-options').offsetWidth - label.closest('.dropdown-options').clientWidth,
  }));
  assert.ok(labelWidth.content <= labelWidth.available + 1, JSON.stringify(labelWidth));
  await wideLabel.hover();
  assert.equal(await wideLabel.getAttribute('title'), null);
  await search.fill(oversizedModel);
  const clipped = list.getByRole('option', { name: oversizedModel, exact: true }).locator('.dropdown-option-label');
  await clipped.hover();
  assert.equal(await clipped.getAttribute('title'), oversizedModel);
  await search.fill(gemini);
  await page.waitForFunction(() => {
    const trigger = document.querySelector('.request-filter .dropdown-trigger');
    const menu = document.querySelector('.request-filter .dropdown-menu');
    return Math.abs(trigger.getBoundingClientRect().left - menu.getBoundingClientRect().left) < 1;
  });
  await list.getByRole('option', { name: gemini, exact: true }).click();
  assert.equal(await page.getByRole('combobox', { name: '模型', exact: true }).textContent(), gemini);
  await page.waitForFunction(model => window.__MODEL_QUERIES__.includes(model), gemini);
  await page.waitForFunction(() => document.querySelectorAll('.request-model .model-label').length === 1);
  assert.equal(await requestLabels.textContent(), gemini);
  await chooseOption(page, '当前 Agent', 'codex');
  const codexModels = await openDropdown(page, '模型');
  await count(codexModels, gemini).getByText('1', { exact: true }).waitFor();
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: '今日', exact: true }).click();
  const todayModels = await openDropdown(page, '模型');
  await count(todayModels, '全部模型').getByText('0', { exact: true }).waitFor();
  await page.waitForFunction(() => document.querySelectorAll('.request-filter .dropdown-option').length === 1);
  await page.keyboard.press('Escape');
  const todayAgents = await openDropdown(page, '当前 Agent');
  assert.equal(await count(todayAgents, 'Codex').textContent(), '0');
  assert.equal(await count(todayAgents, '全部 Agent').textContent(), String(labels.length));
  assert.deepEqual(errors, []);
  console.log(`PASS: model IDs match across session cards, details, usage filters, summaries and request rows (${engine})`);
} finally {
  await browser.close();
}
