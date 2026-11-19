import test from 'node:test';
import assert from 'node:assert/strict';
import { sessionRows, rowVisible } from '../../src/sessionContent.ts';
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
});
