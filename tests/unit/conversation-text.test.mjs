import test from 'node:test';
import assert from 'node:assert/strict';
import { DEFAULT_VISIBILITY, sessionRows, rowVisible, toolDescription, visibleSessionRows } from '../../src/sessionContent.ts';
import { activityItems } from '../../src/activity.ts';
import { eventTextParts, eventMatches, matchRanges } from '../../src/conversationText.ts';

test('only normalized dialogue is searched; literal instruction markup is preserved', () => {
  const quote = '<system-reminder>quoted instructions</system-reminder>';
  const event = { type: 'assistant', message: { content: [null, { type: 'text', text: 12 }, { type: 'thinking', thinking: 'hidden' }, { type: 'tool_result', content: 'hidden' }, { type: 'text', text: quote }] } };
  assert.deepEqual(eventTextParts(event), [quote]);
  assert.equal(eventMatches(event, 'hidden'), false);
  assert.equal(eventMatches(event, 'quoted instructions'), true);
  assert.deepEqual(eventTextParts({type:'user',isMeta:true,message:{content:'hidden'}}), []);
  assert.deepEqual(eventTextParts({type:'context',message:{role:'user',content:'hidden runtime context'}}), []);
  assert.equal(eventMatches({type:'context',message:{role:'developer',content:'hidden runtime context'}}, 'runtime'), false);
  const file = { type: 'user', message: { content: [{ type: 'file', filename: 'attachment.png' }] } };
  assert.deepEqual(eventTextParts(file), []);
  assert.equal(rowVisible(sessionRows([file])[0], { tool: false, thinking: false, context: false }), true);
});

test('literal matching keeps Unicode and original case ranges intact', () => {
  const text = '😀中İABc [x].*';
  assert.deepEqual(matchRanges(text, 'i'), [[3,4]]);
  assert.deepEqual(matchRanges(text, 'i\u0307'), [[3,4]]);
  assert.deepEqual(matchRanges(text, 'aBc'), [[4,7]]);
  assert.deepEqual(matchRanges(text, '😀'), [[0,2]]);
  assert.deepEqual(matchRanges(text, '[x].*'), [[8,13]]);
  assert.deepEqual(matchRanges(text, '  '), []);
  assert.equal(matchRanges('a '.repeat(1000), 'a').length, 200);
});


test('tool results pair in source order and unmatched results stay visible', () => {
  const call = { type: 'tool_use', id: 'reused', name: 'Bash' };
  const result = content => ({ type: 'tool_result', tool_use_id: 'reused', content });
  const assistant = (...content) => ({ type: 'assistant', message: { content } });
  const user = (...content) => ({ type: 'user', message: { content } });
  const rows = sessionRows([
    user(result('older orphan')),
    assistant(call, result('server-side')),
    assistant(call),
    user(result('later client result'), result('extra orphan')),
  ]);
  assert.equal(rows[1].blocks[0].result.content, 'server-side');
  assert.equal(rows[2].blocks[0].result.content, 'later client result');
  assert.equal(rows[0].blocks[0].linked, undefined);
  assert.equal(rows[3].blocks[1].linked, undefined);
  assert.equal(rows[3].blocks[0].linked, true);
  const forked = sessionRows([assistant(call), user(result('child result'))], 1);
  assert.equal(forked[0].blocks[0].result, undefined);
  assert.equal(forked[1].blocks[0].linked, undefined);

  assert.equal(toolDescription({ type: 'tool_use', input: { toolSummary: 'List\n  files' } }), 'List files');
  assert.equal(toolDescription({ type: 'tool_use', input: { Description: 'Subagent task' } }), 'Subagent task');
});

test('activity rows merge across events and adjacent browsing collapses into one group', () => {
  const read = (id, path) => ({ type: 'assistant', message: { content: [{ type: 'tool_use', id, name: 'Read', ops: [{ kind: 'read', path }] }] } });
  const sed = { type: 'assistant', message: { content: [{ type: 'tool_use', id: 'b', name: 'Bash',
    ops: [{ kind: 'shell', intent: 'read', exit_code: 0, steps: [{ kind: 'read', path: '/p/b.ts', program: 'sed' }] }] }] } };
  const rows = sessionRows([
    { type: 'user', message: { content: '看看这两个文件' } },
    { type: 'assistant', message: { content: [{ type: 'text', text: '先读一下' }, { type: 'thinking', thinking: '计划' }] } },
    read('a', '/p/a.ts'),
    { type: 'user', message: { content: [{ type: 'tool_result', tool_use_id: 'a', content: 'x' }] } },
    sed,
    { type: 'assistant', message: { content: [{ type: 'tool_use', id: 'c', name: 'apply_patch', ops: [{ kind: 'edit', path: '/p/a.ts', added: 1, removed: 0 }, { kind: 'search', pattern: 'x' }] }] } },
    { type: 'assistant', message: { content: [{ type: 'text', text: '改好了' }] } },
  ]);
  assert.deepEqual(visibleSessionRows(rows, DEFAULT_VISIBILITY), [
    { index: 0, members: [] }, { index: 1, members: [2, 4, 5] }, { index: 6, members: [] },
  ]);
  const parts = [1, 2, 4, 5].flatMap(i => rows[i].blocks).filter(part => part.content.type !== 'text');
  assert.deepEqual(activityItems(parts).map(item => item.type === 'browse' ? item.items.map(i => i.key) : item.type),
    ['thinking', ['2.0', '4.0'], 'tool', 'tool']);
});
