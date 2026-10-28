import { engine, baseURL, artifactDir } from "../helpers/browser.mjs";
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdirSync } from 'node:fs';
import path from 'node:path';

const browser=engine==='webkit'?await webkit.launch({headless:true}):await chromium.launch({channel:'chrome',headless:true});
const artifacts=artifactDir;mkdirSync(artifacts,{recursive:true});
const errors=[];
const init=({long=false,enabled=false,blocked=false}={})=>async(page)=>{
  await page.addInitScript(({long,enabled,blocked})=>{
    if(enabled)localStorage.setItem('agent-session-manager.markdown.v1','true');
    if(blocked){Storage.prototype.getItem=()=>{throw Error('blocked')};Storage.prototype.setItem=()=>{throw Error('blocked')};}
    const user=(id,text)=>({type:'user',uuid:id,message:{content:text}});
    const assistant=(id,text)=>({type:'assistant',uuid:id,message:{content:text}});
    const markdown='# 汇总标题\n\n这是 **加粗重点** 和 *斜体*、~~删除文字~~、`inlineCode`。\n\n- 列表甲\n- 列表乙\n\n1. 第一步\n2. 第二步\n\n> 引用内容\n\n| 项目 | 状态 |\n| --- | --- |\n| Markdown | 完成 |\n\n- [x] 已完成事项\n- [ ] 待办事项\n\n```ts\nconst codeNeedle = "'+ 'x'.repeat(240)+'";\n```\n\n[参考资料](https://example.com/guide)';
    const messages={
      md:[user('u-md','## 用户标题\n\n- **用户重点**'),assistant('a-md',markdown)],
      plain:[user('u-plain','普通文本\n下一行    多个空格\n数量：2 * 3 = 6'),assistant('a-json','{"key":"**不要格式化**","items":["#不是标题"]}'),assistant('a-xml','<status>不是 Markdown</status>')],
      tools:[user('u-tools','查看日志'),{type:'assistant',uuid:'a-tools',message:{content:[{type:'thinking',thinking:'## 思考标题\n**保持原文**'},{type:'tool_use',id:'call',name:'fixture',deferred:'tool-input'}]}},{type:'user',uuid:'r-tools',message:{content:[{type:'tool_result',tool_use_id:'call',deferred:'tool-output'}]}}],
      safe:[assistant('a-safe','## 安全样例\n\n<script>window.__PWNED__=true</script>\n\n![图片说明](https://markdown-image.invalid/pixel.png)\n\n[危险](javascript:alert(1))\n\n<img src="https://markdown-image.invalid/html.png" onerror="window.__PWNED__=true">')],
    };
    if(long)messages.long=[user('u-first','## 第一条指令'),...Array.from({length:6000},(_,i)=>assistant(`f-${i}`,`## 第 ${i} 条消息\n\n正文 **重点 ${i}**`)),assistant('a-last','# 末尾标题\n\n'+'填充段落 '.repeat(1200)+'\n\n## 搜索段\n\n**最终目标针**\n\n'+'结尾段落 '.repeat(1200))];
    const names={md:'Markdown 示例',plain:'普通文本示例',tools:'工具与思考',safe:'安全渲染示例',long:'长会话示例'};
    const records=Object.keys(messages).map(id=>({sessionId:id,projectDir:'p',cwd:'/tmp/md',title:names[id],model:null,firstTs:null,lastTs:null,userCount:1,assistantCount:1,realUserCount:1,assistantTextCount:1,sizeBytes:100,hasTranscript:true}));
    const state={calls:[],messages};window.__MD__=state;
    window.__TAURI_INTERNALS__={transformCallback:cb=>cb,invoke:async(cmd,args={})=>{
      state.calls.push({cmd,args});
      if(cmd==='list_agents')return[{id:'codex',displayName:'Codex CLI',installed:true,supportsFork:false,deletePolicy:'trash'}];
      if(cmd==='catalog')return{sessions:records,projects:[{dirName:'p',displayPath:'/tmp/md',sessionCount:records.length,lastActivity:null}],warnings:[]};
      if(cmd==='read_session')return{meta:records.find(s=>s.sessionId===args.sessionId),events:messages[args.sessionId]};
      if(cmd==='read_tool_payload')return args.token==='tool-input'?{type:'tool_use',id:'call',name:'fixture',input:{value:'**原样**'}}:{type:'tool_result',tool_use_id:'call',content:'# 结果标题\n**保持原文**'};
      if(cmd==='search_conversations')return{hits:args.targets.flatMap(target=>{const events=messages[target.sessionId];const i=events.findIndex(e=>typeof e.message?.content==='string'&&e.message.content.includes(args.query));return i<0?[]:[{...target,eventIndex:i,eventId:events[i].uuid,role:events[i].type,snippet:args.query}];}),skipped:0,cancelled:false};
      return null;
    }};
  },{long,enabled,blocked});
  page.on('pageerror',e=>errors.push(e.message));
};
let page;
try{
  page=await browser.newPage({viewport:{width:1280,height:800}});await init()(page);
  const external=[];page.on('request',r=>{if(r.url().startsWith('https://markdown-image.invalid'))external.push(r.url());});
  await page.goto(baseURL);await page.locator('.session-card',{hasText:'Markdown 示例'}).click();
  const toggle=page.getByRole('checkbox',{name:'Markdown',exact:true});await toggle.waitFor();assert.equal(await toggle.isChecked(),false);
  assert.equal(await page.locator('.markdown-body').count(),0);
  assert.ok((await page.locator('.msg.assistant').textContent()).startsWith('# 汇总标题'));
  await toggle.check();await page.locator('.msg.assistant .markdown-body h1').waitFor();
  assert.equal(await page.locator('.msg.user .markdown-body h2').textContent(),'用户标题');
  assert.equal(await page.locator('.msg.assistant .markdown-body h1').textContent(),'汇总标题');
  assert.ok(await page.locator('.markdown-body strong').count()>=2);
  assert.equal(await page.locator('.msg.assistant .markdown-body table td').first().textContent(),'Markdown');
  assert.equal(await page.locator('.msg.assistant .markdown-body blockquote').textContent(),'\n引用内容\n');
  assert.ok(await page.locator('.markdown-body pre code.language-ts').count());
  assert.ok(await page.locator('.markdown-body input[disabled]').count());
  const tableBounds=await page.locator('.markdown-table').boundingBox();const bodyBounds=await page.locator('.detail-body').boundingBox();
  assert.ok(tableBounds.x+tableBounds.width<=bodyBounds.x+bodyBounds.width);
  await page.locator('.markdown-body a',{hasText:'参考资料'}).click();
  await page.waitForFunction(()=>window.__MD__.calls.some(c=>c.cmd==='plugin:opener|open_url'&&c.args.url==='https://example.com/guide'));
  assert.equal(page.url(),baseURL);
  await page.screenshot({path:path.join(artifacts,`markdown-enabled-${engine}.png`)});
  console.log(`PASS: optional Markdown headings, lists, table, code, quotes and explicit link opening (${engine})`);

  await page.locator('.session-card',{hasText:'普通文本示例'}).click();assert.equal(await toggle.isChecked(),true);
  await page.locator('.msg.user').waitFor();assert.equal(await page.locator('.markdown-body').count(),0);
  assert.equal(await page.locator('.msg.user .msg-text').textContent(),'普通文本\n下一行    多个空格\n数量：2 * 3 = 6');
  assert.ok((await page.locator('.detail-body').textContent()).includes('**不要格式化**'));
  await page.locator('.session-card',{hasText:'工具与思考'}).click();
  await page.getByRole('button',{name:/工具调用 · fixture/}).click();
  await page.getByRole('button',{name:/思考过程.*思考标题/}).click();
  await page.locator('.collapsible.thinking .markdown-body h2').waitFor();
  assert.equal(await page.locator('.collapsible .markdown-body').count(),2);
  assert.ok((await page.locator('.collapsible.tool pre.code').first().textContent()).includes('**原样**'));
  await toggle.uncheck();assert.equal(await page.locator('.collapsible .markdown-body').count(),0);
  assert.ok((await page.locator('.collapsible-body').allTextContents()).join('\n').includes('**保持原文**'));
  await toggle.check();await page.locator('.collapsible.thinking .markdown-body h2').waitFor();
  assert.equal(await page.locator('.collapsible .markdown-body').count(),2);
  assert.equal(await page.evaluate(()=>window.__MD__.calls.filter(c=>c.cmd==='read_tool_payload').length),2);
  console.log(`PASS: plain text/JSON/XML unchanged; Markdown tool results/thinking render while JSON inputs stay raw (${engine})`);

  await page.locator('.session-card',{hasText:'安全渲染示例'}).click();await page.locator('.markdown-body h2').waitFor();
  assert.equal(await page.locator('.detail-body script,.detail-body img,.detail-body iframe').count(),0);
  assert.equal(await page.locator('a[href^="javascript:"]').count(),0);
  assert.equal(await page.evaluate(()=>window.__PWNED__),undefined);assert.deepEqual(external,[]);
  await page.reload();await page.locator('.session-card',{hasText:'Markdown 示例'}).click();assert.equal(await toggle.isChecked(),true);
  await page.locator('.markdown-body h1').waitFor();
  await toggle.uncheck();assert.equal(await page.locator('.markdown-body').count(),0);
  await page.reload();await page.locator('.session-card',{hasText:'Markdown 示例'}).click();assert.equal(await toggle.isChecked(),false);
  console.log(`PASS: no active HTML or automatic image requests; preference persists across sessions/restarts (${engine})`);
  await page.close();

  page=await browser.newPage({viewport:{width:1280,height:800}});await init({long:true,enabled:true})(page);
  await page.route('**/src/components/MarkdownContent.tsx*',async route=>{await new Promise(resolve=>setTimeout(resolve,450));await route.continue();});
  await page.goto(baseURL);const search=page.getByRole('searchbox');await search.fill('最终目标针');
  await page.locator('.session-search-snippet').waitFor();await page.locator('.session-card',{hasText:'长会话示例'}).click();
  const last=6001;const mark=page.locator(`#ev-${last} mark`);await mark.waitFor();
  await page.locator(`#ev-${last} .markdown-body`).waitFor();
  await page.waitForFunction(i=>{const m=document.querySelector(`#ev-${i} mark`)?.getBoundingClientRect();const b=document.querySelector('.detail-body')?.getBoundingClientRect();return m&&b&&m.top>=b.top&&m.bottom<=b.bottom;},last);
  assert.ok(await page.locator('.event-group').count()<50);
  await page.getByRole('checkbox',{name:'Markdown',exact:true}).uncheck();await mark.waitFor();
  assert.equal(await page.locator(`#ev-${last} .markdown-body`).count(),0);
  await page.getByRole('checkbox',{name:'Markdown',exact:true}).check();await page.locator(`#ev-${last} .markdown-body`).waitFor();
  await page.setViewportSize({width:960,height:600});
  assert.equal(await page.getByRole('checkbox',{name:'Markdown',exact:true}).isVisible(),true);
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  await search.fill('**最终目标针**');await page.locator('.session-search-snippet').waitFor();await page.locator('.session-card',{hasText:'长会话示例'}).click();
  await page.locator(`#ev-${last} mark`).waitFor();assert.equal(await page.locator(`#ev-${last} mark`).textContent(),'**最终目标针**');
  assert.equal(await page.locator(`#ev-${last} .markdown-body`).count(),0);
  console.log(`PASS: bounded 6002-message DOM, delayed rendering, Markdown/raw search highlights and narrow layout (${engine})`);
  await page.close();

  page=await browser.newPage();await init({blocked:true})(page);await page.goto(baseURL);await page.locator('.session-card',{hasText:'Markdown 示例'}).click();
  await page.getByRole('checkbox',{name:'Markdown',exact:true}).check();await page.locator('.markdown-body h1').waitFor();assert.deepEqual(errors,[]);
  console.log(`PASS: blocked storage does not prevent rendering; no browser errors (${engine})`);
  await page.close();
  page=await browser.newPage();await init()(page);
  await page.route('**/src/components/MarkdownContent.tsx*',route=>route.abort());
  await page.goto(baseURL);await page.locator('.session-card',{hasText:'Markdown 示例'}).click();
  await page.getByRole('checkbox',{name:'Markdown',exact:true}).check();
  await page.waitForFunction(()=>!document.querySelector('[data-markdown-pending]')&&document.querySelector('.msg.assistant')?.textContent.includes('# 汇总标题'));
  await page.locator('.session-card',{hasText:'普通文本示例'}).click();await page.getByText('普通文本',{exact:false}).first().waitFor();
  assert.equal(await page.getByRole('checkbox',{name:'Markdown',exact:true}).isChecked(),true);
  console.log(`PASS: unavailable renderer falls back to original text without losing navigation (${engine})`);
}catch(error){await page?.screenshot({path:path.join(artifacts,`markdown-failure-${engine}.png`)}).catch(()=>{});throw error;}
finally{await browser.close();}
