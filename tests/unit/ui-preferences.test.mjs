import test from 'node:test';
import assert from 'node:assert/strict';
import { createUiPreferenceStore } from '../../src/uiPreferences.ts';

const key = 'agent-session-manager.ui-preferences.v1';
const markdownKey = 'agent-session-manager.markdown.v1';
const widthsKey = 'agent-session-manager.panel-widths.v1';

function storage(seed = {}) {
  const entries = new Map(Object.entries(seed));
  return {
    getItem: id => entries.get(id) ?? null,
    setItem: (id, value) => entries.set(id, value),
  };
}

test('legacy Markdown and panel widths survive the first write and a subsequent reload', () => {
  const disk = storage({ [markdownKey]: 'true', [widthsKey]: '{"sidebar":280,"sessions":390}' });
  const prefs = createUiPreferenceStore(disk);
  assert.deepEqual(prefs.get('panelWidths'), { sidebar: 280, sessions: 390 });
  assert.equal(prefs.get('conversation').markdown, true);
  assert.equal(disk.getItem(key), null, 'loading legacy values does not rewrite storage');
  prefs.set('conversation', current => ({ ...current, markdown: false }));
  const reopened = createUiPreferenceStore(disk);
  assert.equal(reopened.get('conversation').markdown, false);
  assert.deepEqual(reopened.get('panelWidths'), { sidebar: 280, sessions: 390 });
  assert.equal(disk.getItem(markdownKey), 'true');
  assert.equal(disk.getItem(widthsKey), '{"sidebar":280,"sessions":390}');
});

test('invalid values fall back independently; a blank project remains a valid filter', () => {
  const disk = storage({ [key]: JSON.stringify({
    agents: { local: 'all', usage: 12 },
    sessionList: { includeEmpty: true, includeArchived: 'true' },
    conversation: { context: true, thinking: 0 },
    panelWidths: { sidebar: -10, sessions: 470 },
    usage: { tab: 'prices', period: 'lifetime', priceStatus: 'custom', requestPageSize: 10 },
    usageAnalysis: { trendMetric: 'cost', trendGroup: 'none', dimension: 'projects', shareMetric: false },
    requestFilters: { codex: { model: false, project: '', status: 'not-a-status' }, all: { model: 'demo', status: 'failed' } },
  }) });
  const prefs = createUiPreferenceStore(disk);
  assert.deepEqual(prefs.get('agents'), { local: null, usage: 'all' });
  assert.deepEqual(prefs.get('sessionList'), { includeEmpty: true, includeArchived: false });
  assert.deepEqual(prefs.get('conversation'), { tool: true, thinking: true, context: true, markdown: false });
  assert.deepEqual(prefs.get('panelWidths'), { sidebar: 220, sessions: 470 });
  assert.deepEqual(prefs.get('usage'), { tab: 'prices', period: 'month', priceStatus: 'custom', requestPageSize: 50 });
  assert.deepEqual(prefs.get('usageAnalysis'), { trendMetric: 'cost', trendGroup: 'category', dimension: 'projects', shareMetric: 'tokens' });
  assert.deepEqual(prefs.get('requestFilters'), {
    codex: { model: null, project: '', status: null }, all: { model: 'demo', project: null, status: 'failed' },
  });
  disk.setItem(key, '{broken');
  assert.deepEqual(createUiPreferenceStore(disk).get('sessionList'), { includeEmpty: false, includeArchived: false });
});

test('unavailable storage only prevents persistence across restarts, not live preference updates', () => {
  const unavailable = () => { throw Error('Storage unavailable'); };
  const prefs = createUiPreferenceStore({ getItem: unavailable, setItem: unavailable });
  const before = prefs.get('usage');
  let observed;
  const unsubscribe = prefs.subscribe(() => { observed = prefs.get('usage'); });
  prefs.set('usage', current => ({ ...current, period: 'today' }));
  assert.equal(observed.period, 'today');
  assert.equal(prefs.get('usage').period, 'today');
  assert.equal(before.period, 'month');
  unsubscribe();
});
