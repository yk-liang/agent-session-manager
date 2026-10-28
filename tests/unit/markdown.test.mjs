import test from 'node:test';
import assert from 'node:assert/strict';
import { canRenderMarkdown, externalMarkdownLink, highlightMarkdown } from '../../src/markdown.ts';
import { readMarkdownPreference, saveMarkdownPreference } from '../../src/markdownPreference.ts';

test('real Markdown is recognized while plain text, raw JSON and tags retain their original form',()=>{
  for(const text of ['# 标题','标题\n===','**加粗**','*斜体*','`code`','- 第一项\n- 第二项','1. 第一项\n2. 第二项','> 引用','```js\nlet a=1\n```','| a | b |\n|---|---|\n|1|2|','~~删除~~','[1](https://example.com)']) assert.equal(canRenderMarkdown(text),true,text);
  for(const text of ['纯文本\n保留换行','第一段\n\n第二段','mode_id_abc_def','<environment_context>test</environment_context>','{"value":"**literal**"}','["**literal**"]','2026-10-19 [INFO] no format','https://example.com/path','联系 user@example.com','[ref]: https://example.com']) assert.equal(canRenderMarkdown(text),false,text);
});

test('searching Markdown syntax or hidden link addresses falls back to the searchable source',()=>{
  assert.equal(canRenderMarkdown('**目标**','目标'),true);
  assert.equal(canRenderMarkdown('**目标**','**目标**'),false);
  assert.equal(canRenderMarkdown('[参考](https://example.com/private-path)','private-path'),false);
  assert.equal(canRenderMarkdown('```rust\nconst needle=1\n```','needle'),true);
});

test('highlighting inserts text-safe Unicode marks into rendered nodes without changing URLs or code',()=>{
  const tree={type:'root',children:[{type:'element',tagName:'a',properties:{href:'https://example.com/目标针'},children:[{type:'text',value:'前文 😀 目标针 后文'}]},{type:'element',tagName:'code',children:[{type:'text',value:'目标针'}]}]};
  highlightMarkdown({query:'目标针'})(tree);
  assert.equal(tree.children[0].properties.href,'https://example.com/目标针');
  assert.equal(tree.children[0].children[1].tagName,'mark');
  assert.equal(tree.children[0].children[1].children[0].value,'目标针');
  assert.equal(tree.children[1].children[0].tagName,'mark');
  assert.equal(externalMarkdownLink('javascript:alert(1)'),false);
  assert.equal(externalMarkdownLink('file:///etc/passwd'),false);
  assert.equal(externalMarkdownLink('https://example.com'),true);
});

test('Markdown preference survives reload and storage failure cannot block the checkbox',()=>{
  const old=globalThis.localStorage;const values=new Map();
  try {
    globalThis.localStorage={getItem:key=>values.get(key)??null,setItem:(key,value)=>values.set(key,value)};
    assert.equal(readMarkdownPreference(),false);saveMarkdownPreference(true);assert.equal(readMarkdownPreference(),true);
    saveMarkdownPreference(false);assert.equal(readMarkdownPreference(),false);
    globalThis.localStorage={getItem:()=>{throw Error('blocked')},setItem:()=>{throw Error('blocked')}};
    assert.equal(readMarkdownPreference(),false);assert.doesNotThrow(()=>saveMarkdownPreference(true));
  } finally {if(old===undefined)delete globalThis.localStorage;else globalThis.localStorage=old;}
});
