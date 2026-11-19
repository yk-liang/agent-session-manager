import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { engine, baseURL } from '../helpers/browser.mjs';
import { openDropdown } from '../helpers/dropdown.mjs';

const opus = 'claude-opus-5-5';
const gemini = 'gemini-4-neon';
const geminiAlias = 'Gemini 4 Neon (XHigh)';
const browser = engine === 'webkit' ? await webkit.launch({ headless: true }) : await chromium.launch({ channel: 'chrome', headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(({ opus, gemini, geminiAlias }) => {
    const agent = 'antigravity';
    const models = [[opus, 'Claude Opus 5.5'], [gemini, geminiAlias], ['unknown', 'Unknown model']];
    const session = { sessionId: 'fixture-session', projectDir: 'fixture', cwd: '/tmp/fixture', title: 'Model fixture',
      model: gemini, modelName: geminiAlias, realUserCount: 1, assistantTextCount: 1, sizeBytes: 2000, hasTranscript: true };
    const tokens = { inputTokens: 100, outputTokens: 10, cacheReadTokens: 80, cacheWriteTokens: 0, reasoningTokens: 0,
      unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true, cacheReadRecorded: true };
    const rows = models.map(([model, modelName]) => ({ ...tokens, agent, model, modelName, bucketStart: Date.now() - 1000,
      date: '', totalTokens: 110, requestCount: 1, requestCountIncomplete: false, partial: false }));
    const events = models.map(([model, modelName], index) => ({ ...tokens, id: `request-${index}`, agent, model, modelName,
      time: Date.now() - (index + 1) * 1000, session: session.sessionId, status: 'success', partial: false, request: {} }));
    const queries = window.__MODEL_QUERIES__ = [];
    window.__TAURI_INTERNALS__ = { transformCallback: cb => cb, invoke: async (command, args = {}) => {
      if (command === 'list_agents') return [{ id: agent, displayName: 'Antigravity', installed: true, deletePolicy: 'readOnly' }];
      if (command === 'catalog') return { sessions: [session], warnings: [], projects: [
        { dirName: session.projectDir, displayPath: session.cwd, sessionCount: 1, lastActivity: null },
      ] };
      if (command === 'read_session') return { meta: session, events: [{ type: 'user', message: { content: 'Model fixture' } }] };
      if (command === 'search_conversations') return { hits: [], skipped: 0, cancelled: false };
      if (command === 'read_usage_summary') return { rows, health: {}, syncedAgents: [agent], revision: '1' };
      if (command === 'read_request_events') {
        queries.push(args.query.model);
        const page = events.filter(event => !args.query.model || event.model === args.query.model);
        return { events: page, total: page.length, page: 1, pageSize: 50, revision: '1' };
      }
      return null;
    }};
  }, { opus, gemini, geminiAlias });

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
  const requestLabels = page.locator('.request-model .model-label');
  const summaryLabels = page.locator('.usage-models tbody .model-label');
  await requestLabels.first().waitFor();
  await summaryLabels.first().waitFor();
  const labels = [opus, gemini, '未知模型'].sort();
  assert.deepEqual((await requestLabels.allTextContents()).sort(), labels);
  assert.deepEqual((await summaryLabels.allTextContents()).sort(), labels);
  const list = await openDropdown(page, '模型');
  assert.deepEqual((await list.getByRole('option').allTextContents()).sort(), [...labels, '全部模型'].sort());
  const search = page.getByRole('combobox', { name: '搜索模型', exact: true });
  await search.fill(gemini);
  await list.getByRole('option', { name: gemini, exact: true }).click();
  await page.waitForFunction(model => window.__MODEL_QUERIES__.includes(model), gemini);
  await page.waitForFunction(() => document.querySelectorAll('.request-model .model-label').length === 1);
  assert.equal(await requestLabels.textContent(), gemini);
  assert.equal(await summaryLabels.textContent(), gemini);
  assert.equal(await page.getByRole('combobox', { name: '模型', exact: true }).textContent(), gemini);
  assert.deepEqual(errors, []);
  console.log(`PASS: model IDs match across session cards, details, usage filters, summaries and request rows (${engine})`);
} finally {
  await browser.close();
}
