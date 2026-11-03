// Public screenshot data only. Never import local Agent logs, settings or credentials here.
export const screenshotTime = '2026-11-02T10:30:00-08:00';
const now = Date.parse(screenshotTime);
const names = { qoder: 'Qoder CLI', claude: 'Claude Code', codex: 'Codex', opencode: 'OpenCode', pi: 'Pi' };
const projects = ['atlas-notes', 'design-system', 'weather-kit', 'docs-site'];
const titles = ['给笔记应用添加全文搜索', '优化长列表的滚动性能', '统一按钮与输入框样式', '补充搜索模块的单元测试', '整理项目的快速开始文档', '为天气卡片添加加载状态', '支持 Markdown 表格预览', '完善深色主题的对比度'];
const models = { codex: 'gpt-5-codex', claude: 'claude-sonnet-4-5', opencode: 'qwen3-coder', qoder: 'qwen3-coder', pi: 'claude-sonnet-4-5' };
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
  const costUsd = agent === 'codex' ? null : Math.round((inputTokens * 0.000003 + outputTokens * 0.000015) * 1e6) / 1e6;
  return {
    id: `demo-request-${String(i).padStart(4, '0')}`, agent, time,
    session: sessionId(i % titles.length), model: models[agent], modelName: models[agent],
    status: 'success', partial: false, inputTokens, outputTokens, cacheReadTokens,
    cacheWriteTokens: 0, reasoningTokens: agent === 'codex' ? Math.floor(outputTokens * 0.35) : 0,
    unclassifiedTokens: 0, inputRecorded: true, outputRecorded: true,
    cacheReadRecorded: true, cacheWriteRecorded: false, reasoningRecorded: agent === 'codex',
    outputTps: outputTokens / (latencyMs / 1000),
    request: {
      requestId: `demo-response-${i}`, provider: agent === 'codex' ? 'openai' : agent === 'claude' || agent === 'pi' ? 'anthropic' : 'example',
      project: `/Users/demo/Projects/${projects[i % projects.length]}`,
      client: agent === 'codex' ? 'Codex Desktop' : names[agent], kind: null,
      effort: agent === 'codex' ? 'high' : null, tier: null, version: null, source: agent === 'codex' ? 'vscode' : 'cli',
      stop: 'end_turn', httpStatus: null, startedAt: time - latencyMs, finishedAt: time,
      timing: agent === 'codex' ? 'estimated' : 'recorded', latencyMs, ttftMs: null, ttftScope: null,
      scope: 'request', costUsd,
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
    id, displayName, installed: true, supportsFork: id === 'codex', supportsModelNames: false,
    supportsFastCatalog: false, dataDir: `/Users/demo/.${id}`, deletePolicy: id === 'opencode' ? 'permanent' : 'trash',
  })),
  catalog: {
    sessions, warnings: [],
    projects: projects.map(dirName => ({
      dirName, displayPath: `/Users/demo/Projects/${dirName}`,
      sessionCount: sessions.filter(s => s.projectDir === dirName).length,
      lastActivity: sessions.find(s => s.projectDir === dirName).lastTs,
    })),
  },
  conversation, events,
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

// Serialized into an isolated browser context; every native call is intercepted.
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
      case 'resolve_model_names': return Object.fromEntries(args.models.map(model => [model, model]));
      case 'collector_status': return { desired: false, running: false, authorization: 'signingRequired' };
      case 'sync_request_events': return { updatedAt: Date.now(), checkedFiles: 0, changedFiles: 0, bytesRead: 0 };
      case 'read_usage_summary': {
        const { from, to, bounds } = args.query;
        return {
          revision: 'demo-1', syncedAgents: fixture.agents.map(a => a.id), health: {},
          rows: fixture.events.filter(e => (from == null || e.time >= from) && (to == null || e.time < to)).map(e => ({
            ...e, date: new Date(e.time).toISOString().slice(0, 10),
            bucketStart: bounds.find((start, i) => e.time >= start && e.time < bounds[i + 1]) ?? null,
            totalTokens: e.inputTokens + e.outputTokens, requestCount: 1, requestCountIncomplete: false,
            costUsd: e.request.costUsd, costIncomplete: e.request.costUsd == null,
            cacheReadIncomplete: false, cacheRateUnavailable: false,
          })),
        };
      }
      case 'read_request_events': {
        const q = args.query;
        const scoped = fixture.events.filter(e => (!q.agent || q.agent === e.agent) && (q.from == null || e.time >= q.from) && (q.to == null || e.time < q.to));
        const filtered = scoped.filter(e => (!q.model || q.model === e.model) && (!q.status || q.status === e.status));
        const pageSize = q.limit ?? 50, page = q.page ?? 1;
        return { events: filtered.slice((page - 1) * pageSize, page * pageSize), total: filtered.length, page, pageSize, revision: 'demo-1', models: [...new Set(scoped.map(e => e.model))] };
      }
      case 'plugin:event|listen': return 1;
      case 'plugin:event|unlisten':
      case 'watch_sessions':
      case 'cancel_usage':
      case 'cancel_usage_summary':
      case 'cancel_request_events':
      case 'cancel_catalog_refinement':
      case 'cancel_conversation_search':
      case 'set_config_guard': return null;
      default: throw Error(`Unexpected screenshot command: ${command}`);
    }
  } };
}
