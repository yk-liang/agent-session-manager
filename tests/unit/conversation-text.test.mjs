import test from 'node:test';
import assert from 'node:assert/strict';
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
