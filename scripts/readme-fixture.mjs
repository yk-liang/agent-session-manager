// Synthetic sessions, usage and configuration for public screenshots.
export const screenshotTime = '2026-11-02T10:30:00-08:00';
const now = Date.parse(screenshotTime);
const names = { qoder: 'Qoder', claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', pi: 'Pi' };
const projects = ['atlas-notes', 'design-system', 'weather-kit', 'docs-site'];
const titles = ['给笔记应用添加全文搜索', '优化长列表的滚动性能', '统一按钮与输入框样式', '补充搜索模块的单元测试', '整理项目的快速开始文档', '为天气卡片添加加载状态', '支持 Markdown 表格预览', '完善深色主题的对比度'];
const models = { codex: 'gpt-5-codex', claude: 'claude-sonnet-4-5', opencode: 'qwen3-coder', qoder: 'qwen3-coder', pi: 'claude-sonnet-4-5' };
const prices = {
  'gpt-5-codex': { source: 'models.dev', provider: 'openai', modelId: 'gpt-5-codex', currency: 'USD', rates: { input: 1.5, output: 10, cacheRead: 0.15 } },
  'claude-sonnet-4-5': { source: 'models.dev', provider: 'anthropic', modelId: 'claude-sonnet-4-5', currency: 'USD', rates: { input: 3, output: 15, cacheRead: 0.3, cacheWrite: 3.75 } },
  'qwen3-coder': { source: 'custom', currency: 'CNY', originalRates: { input: 7, output: 28, cacheRead: 0.7 }, rates: { input: 1, output: 4, cacheRead: 0.1 } },
};
const sessionId = i => `47bdf210-0000-4000-8000-${String(i + 1).padStart(12, '0')}`;

const sessions = titles.map((title, i) => ({
  sessionId: sessionId(i), projectDir: projects[i % projects.length],
  cwd: `/Users/demo/Projects/${projects[i % projects.length]}`, title, model: models.codex,
  version: null, gitBranch: i === 0 ? 'feature/search' : 'main', entrypoint: 'Codex Desktop',
  firstTs: new Date(now - (i + 1) * 3_600_000).toISOString(),
  lastTs: new Date(now - (i * 5_400_000 + 720_000)).toISOString(),
  userCount: 2, assistantCount: 2, realUserCount: 2, assistantTextCount: 2,
  sizeBytes: 158_400 + i * 72_000, hasTranscript: true,
}));

const conversation = [
  { type: 'context', context: { kind: 'environment', label: '项目上下文' }, message: { content: 'Synthetic project context for the public screenshots.' } },
  { type: 'user', message: { content: '为 Atlas Notes 增加全文搜索。支持关键词高亮、键盘导航，保留现有的深色界面。' } },
  { type: 'assistant', message: { content: '## 搜索体验已完成\n\n现在可以同时搜索**笔记标题与正文**，并直接跳转到命中位置。\n\n| 功能 | 交互 |\n| --- | --- |\n| 即时搜索 | 输入关键词后自动更新结果 |\n| 关键词高亮 | 标题与正文片段同步标记 |\n| 键盘导航 | ↑ ↓ 切换结果，Enter 打开笔记 |\n| 空结果 | 保留搜索词，提示尝试其他关键词 |\n\n搜索面板复用了现有的颜色和间距，浅色与深色主题均保持一致。' } },
  { type: 'user', message: { content: '再补充快捷键，让我可以随时打开搜索。' } },
  { type: 'assistant', message: { content: '已加入 **⌘ K** 快捷键。打开时自动聚焦搜索框，按 **Esc** 关闭并返回之前的笔记。\n\n- [x] 标题和正文搜索\n- [x] 高亮与键盘导航\n- [x] 快捷键与焦点恢复\n- [x] 搜索、空结果与关闭行为的测试' } },
].map((event, i) => ({ ...event, uuid: `demo-message-${i}`, timestamp: new Date(now - (8 - i) * 60_000).toISOString() }));

// Summary cards, chart buckets and model totals are derived from these same requests.
const requestDays = [25, 38, 16, 30, 12, 41, 18].flatMap((count, day) => Array.from({ length: count }, (_, slot) => ({ day, slot })));
const events = requestDays.map(({ day, slot }, i) => {
  const agent = ['codex', 'claude', 'codex', 'opencode', 'qoder', 'pi'][i % 6];
  const time = now - day * 86_400_000 - (slot * 6 + 5) * 60_000;
  const inputTokens = 24_000 + ((i * 7919) % 72_000);
  const outputTokens = 450 + ((i * 137) % 2800);
  const cacheReadTokens = Math.floor(inputTokens * (0.78 + (i % 16) / 100));
  const latencyMs = 13_000 + (i % 12) * 1900;
  const price = prices[models[agent]];
  const { input: inputRate, output: outputRate, cacheRead: cacheReadRate, cacheWrite: cacheWriteRate = inputRate } = price.rates;
  const inputUsd = (inputTokens - cacheReadTokens) * inputRate / 1e6;
  const cacheReadUsd = cacheReadTokens * cacheReadRate / 1e6;
  const outputUsd = outputTokens * outputRate / 1e6;
  return {
    id: `demo-request-${String(i).padStart(4, '0')}`, agent, time,
    session: sessionId(i % titles.length), model: models[agent],
    status: 'success', partial: false, inputTokens, outputTokens, cacheReadTokens,
    cacheWriteTokens: 0, reasoningTokens: agent === 'codex' ? Math.floor(outputTokens * 0.35) : 0,
    unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true,
    cacheReadRecorded: true, cacheWriteRecorded: false, reasoningRecorded: agent === 'codex',
    outputTps: outputTokens / (latencyMs / 1000),
    cost: {
      source: price.source, currency: price.currency, totalUsd: inputUsd + cacheReadUsd + outputUsd,
      inputRate, outputRate, cacheReadRate, cacheWriteRate, inputUsd, cacheReadUsd, outputUsd, cacheWriteUsd: 0, longContext: false,
    },
    request: {
      requestId: `demo-response-${i}`, provider: agent === 'codex' ? 'openai' : agent === 'claude' || agent === 'pi' ? 'anthropic' : 'example',
      project: `/Users/demo/Projects/${projects[i % projects.length]}`,
      client: agent === 'codex' ? 'Codex Desktop' : names[agent], kind: null,
      effort: agent === 'codex' ? 'high' : null, tier: null, version: null, source: agent === 'codex' ? 'appServer' : 'cli',
      stop: 'end_turn', httpStatus: null, startedAt: time - latencyMs, finishedAt: time,
      timing: agent === 'codex' ? 'codex_boundary' : 'recorded', latencyMs, ttftMs: null, ttftScope: null,
      scope: 'request',
    },
  };
});

const configContent = 'model = "gpt-5-codex"\nmodel_reasoning_effort = "high"\n\n[projects."/Users/demo/Projects/atlas-notes"]\ntrust_level = "trusted"\n';
const configFile = {
  key: 'config.toml', label: 'config.toml', path: '/Users/demo/.codex/config.toml',
  present: true, content: configContent, revision: 'demo-config-1', validation: { valid: true, error: null },
};

export const screenshotFixture = {
  agents: Object.entries(names).map(([id, displayName]) => ({
    id, displayName, installed: true, supportsFork: id === 'codex',
    dataDir: `/Users/demo/.${id}`, deletePolicy: id === 'opencode' ? 'permanent' : 'trash',
  })),
  catalog: {
    sessions, warnings: [],
    projects: projects.map(dirName => ({
      dirName, displayPath: `/Users/demo/Projects/${dirName}`,
      sessionCount: sessions.filter(s => s.projectDir === dirName).length,
      lastActivity: sessions.find(s => s.projectDir === dirName).lastTs,
    })),
  },
  conversation, events, prices,
  workspace: {
    agent: 'codex', revision: 'demo-workspace-1', files: [configFile],
    profiles: ['日常开发', '深入推理', '快速原型'].map((name, i) => ({
      name, agent: 'codex', createdAt: now - (i + 1) * 86_400_000,
      files: [{ key: configFile.key, label: configFile.label, present: true }],
    })),
    matchingProfiles: ['日常开发'],
    overview: { model: 'gpt-5-codex', provider: 'openai', endpoint: null, mcpServers: [] },
    backups: [
      { id: 'demo-backup-1', createdAt: now - 3_600_000, label: '应用配置「日常开发」', files: ['config.toml'], legacy: false },
      { id: 'demo-backup-2', createdAt: now - 86_400_000, label: '编辑 config.toml', files: ['config.toml'], legacy: false },
    ],
    warnings: [],
  },
};

// Runs in the browser via addInitScript.
export function installScreenshotFixture(fixture) {
  localStorage.setItem('agent-session-manager.markdown.v1', 'true');
  window.__SCREENSHOT_CALLS__ = [];
  window.__TAURI_INTERNALS__ = { transformCallback: callback => callback, invoke: async (command, args = {}) => {
    window.__SCREENSHOT_CALLS__.push(command);
    switch (command) {
      case 'list_agents': return fixture.agents;
      case 'catalog': return fixture.catalog;
      case 'read_session': return { meta: fixture.catalog.sessions.find(s => s.sessionId === args.sessionId), events: fixture.conversation };
      case 'get_config_workspace': return fixture.workspace;
      case 'usage_sync_status': return { sequence: 1, revision: 1, updatedAt: Date.now(), syncing: [], progress: null,
        recollecting: [], failedAgents: [], unavailable: false };
      case 'read_usage_summary': {
        const { from, to, bounds } = args.query;
        const rows = fixture.events.filter(e => (from == null || e.time >= from) && (to == null || e.time < to)).map(e => ({
          ...e, date: new Date(e.time).toISOString().slice(0, 10),
          bucketStart: bounds.find((start, i) => e.time >= start && e.time < bounds[i + 1]) ?? null,
          totalTokens: e.inputTokens + e.outputTokens, recordCount: 1, requestCount: 1, requestCountIncomplete: false,
          successCount: Number(e.status === 'success'), failedCount: Number(e.status === 'failed'),
          costUsd: e.cost.totalUsd, costIncomplete: false,
          [e.cost.source === 'custom' ? 'customCostUsd' : 'referenceCostUsd']: e.cost.totalUsd,
          cacheReadIncomplete: false, cacheRateUnavailable: false,
        }));
        return {
          revision: 'demo-1', syncedAgents: fixture.agents.map(a => a.id), rows,
          projects: rows.map(row => ({ ...row, key: row.request.project ?? '' })),
          providers: rows.map(row => ({ ...row, key: row.request.provider ?? '' })),
        };
      }
      case 'read_request_events': {
        const q = args.query;
        const scoped = fixture.events.filter(e => (!q.agent || q.agent === e.agent) && (q.from == null || e.time >= q.from) && (q.to == null || e.time < q.to));
        const match = (event, facet) =>
          (facet === "models" || q.model == null || event.model === q.model) &&
          (facet === "projects" || q.project == null || (event.request.project ?? "") === q.project) &&
          (facet === "results" || q.status == null || event.status === q.status);
        const filtered = scoped.filter(event => match(event));
        const filters = q.includeFilters ? Object.fromEntries(Object.entries({
          models: event => event.model,
          projects: event => event.request.project ?? "",
          results: event => event.status,
        }).map(([facet, key]) => {
          const counts = {};
          for (const event of scoped.filter(event => match(event, facet))) {
            const id = key(event);
            counts[id] = (counts[id] ?? 0) + 1;
          }
          return [facet, counts];
        })) : undefined;
        const pageSize = q.limit ?? 50, page = q.page ?? 1;
        return { events: filtered.slice((page - 1) * pageSize, page * pageSize), total: filtered.length,
          hasCacheWrites: filtered.some(event => event.cacheWriteTokens > 0),
          hasServiceTiers: filtered.some(event => event.request.tier?.trim()), page, pageSize, revision: 'demo-1', filters };
      }
      case 'get_model_prices': return {
        models: [
          { model: 'gpt-5-codex', price: fixture.prices['gpt-5-codex'] },
          { model: 'claude-sonnet-4-5', price: fixture.prices['claude-sonnet-4-5'] },
          { model: 'qwen3-coder', price: fixture.prices['qwen3-coder'] },
        ],
        catalog: {
          revision: 1,
          sources: [
            { id: 'models.dev', name: 'Models.dev', url: 'https://models.dev/api.json', fetchedAt: Date.now(), modelCount: 120, pricedCount: 100, stale: false, error: null },
            { id: 'litellm', name: 'LiteLLM', url: 'https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json', fetchedAt: Date.now(), modelCount: 200, pricedCount: 160, stale: false, error: null },
          ],
          customCount: 1,
          exchangeRateCnyPerUsd: 7,
        },
      };
      case 'search_reference_prices': return [];
      case 'refresh_reference_prices':
      case 'set_custom_price':
      case 'delete_custom_price': return { revision: 2, sources: [], customCount: 1, exchangeRateCnyPerUsd: 7 };
      case 'plugin:event|listen': return 1;
      case 'plugin:event|unlisten':
      case 'watch_sessions':
      case 'check_usage_now':
      case 'cancel_usage_summary':
      case 'cancel_request_events':
      case 'cancel_catalog':
      case 'cancel_conversation_search':
      case 'set_config_guard': return null;
      default: throw Error(`Unexpected screenshot command: ${command}`);
    }
  } };
}
