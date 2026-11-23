import { engine, baseURL } from "../helpers/browser.mjs";
// 首屏不等待统计；长会话 DOM 有界，搜索/目录仍可到达末尾，工具正文按需获取。
import { webkit, chromium } from 'playwright';
import assert from 'node:assert/strict';

const browser = engine === 'webkit' ? await webkit.launch({headless:true}) : await chromium.launch({channel:'chrome',headless:true});
try {
  const page = await browser.newPage({viewport:{width:1280,height:800}});
  const errors=[]; page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{
    const meta={sessionId:'long',projectDir:'p',cwd:'/tmp/long',title:'长会话性能回归',model:'logged-codex-model',firstTs:null,lastTs:null,userCount:2,assistantCount:6000,realUserCount:2,assistantTextCount:5999,sizeBytes:400000000,hasTranscript:true};
    const events=[{type:'user',uuid:'first',message:{content:'第一条指令'}},
      {type:'assistant',uuid:'tool',message:{model:'earlier-codex-model',content:[{type:'tool_use',id:'call',name:'shell',deferred:'input'}]}},
      {type:'user',uuid:'result',message:{content:[{type:'tool_result',tool_use_id:'call',is_error:true,deferred:'output'}]}},
      ...Array.from({length:6000},(_,i)=>({type:'assistant',uuid:`a-${i}`,message:{content:`消息 ${i}\n`+'正文示例\n'.repeat(10)}})),
      {type:'user',uuid:'last',message:{content:'末尾指令 '+ '长文本'.repeat(3000)+' 唯一定位针 '+ '结尾'.repeat(2000)}}];
    const state={calls:[],refining:false,refined:false,release:null};window.__PERF__=state;
    const catalog=statsPending=>({sessions:[{...meta,model:statsPending?null:meta.model,statsPending}],projects:[{dirName:'p',displayPath:'/tmp/long',sessionCount:1,lastActivity:null}],warnings:[]});
    window.__TAURI_INTERNALS__={transformCallback:cb=>cb,invoke:async(cmd,args={})=>{
      state.calls.push({cmd,args});
      if(cmd==='list_agents')return [{id:'codex',displayName:'Codex',installed:true,deletePolicy:'trash'}];
      if(cmd==='catalog')return catalog(!state.refined);
      if(cmd==='refine_catalog'){state.refining=true;await new Promise(resolve=>state.release=resolve);state.refined=true;return catalog(false);}
      if(cmd==='read_session')return {meta,events};
      if(cmd==='get_config_workspace')return {agent:args.agent,revision:'fixture',files:[],profiles:[],matchingProfiles:[],overview:{model:null,provider:null,endpoint:null,mcpServers:[]},backups:[],warnings:[]};
      if(cmd==='read_tool_payload')return args.token==='input'?{type:'tool_use',id:'call',name:'shell',input:{cmd:'loaded on demand'}}:{type:'tool_result',tool_use_id:'call',content:'result on demand',is_error:true};
      if(cmd==='search_conversations')return {hits:[{sessionId:'long',projectDir:'p',role:'user',eventIndex:events.length-1,eventId:'last',snippet:'唯一定位针'}],skipped:0,cancelled:false};
      return null;
    }};
  });
  await page.goto(baseURL);
  await page.locator('.session-card').waitFor();
  await page.waitForFunction(()=>window.__PERF__.refining);
  assert.ok(!(await page.locator('.session-card-sub').textContent()).includes('条消息'));
  assert.equal(await page.locator('.session-card .model-label').count(),0);
  const start=Date.now();await page.locator('.session-card').click();
  await page.locator('.event-group').first().waitFor();
  console.log(`PASS: first detail render ${Date.now()-start} ms with 6004 events (${engine})`);
  assert.ok(await page.locator('.event-group').count()<50);
  await page.getByRole('button',{name:'更多信息',exact:true}).click();
  assert.deepEqual(await page.locator('.session-info-popover .model-label').allTextContents(),
    ['logged-codex-model', 'earlier-codex-model']);
  assert.equal(await page.locator('#ev-1').getAttribute('title'), '模型：earlier-codex-model');
  await page.getByRole('button',{name:'更多信息',exact:true}).click();
  assert.equal(await page.evaluate(()=>window.__PERF__.calls.filter(c=>c.cmd==='read_tool_payload').length),0);
  const tool = page.getByRole('button',{name:/^工具调用.*失败.*shell$/});
  assert.equal(await tool.getAttribute('aria-expanded'), 'false');
  await tool.click();
  await page.getByText('result on demand',{exact:true}).waitFor();
  assert.equal(await page.evaluate(()=>window.__PERF__.calls.filter(c=>c.cmd==='read_tool_payload').length),2);
  await page.locator('.toc-item').last().click();
  await page.locator('#ev-6003').waitFor();
  assert.ok(await page.locator('.event-group').count()<50);
  await page.locator('.toc-item').first().click();
  await page.getByText('result on demand',{exact:true}).waitFor();
  const currentPrompt = page.locator('.toc-item[aria-current="location"]');
  await page.waitForFunction(() => document.querySelector('.toc-item[aria-current="location"]')?.textContent.includes('第一条指令'));
  await page.getByRole('button', { name: '收起导览', exact: true }).click();
  assert.equal(await page.locator('.toc').getAttribute('data-collapsed'), 'true');
  await page.getByRole('button', { name: '展开导览', exact: true }).click();
  await page.locator('.toc-item').first().focus();
  await page.keyboard.press('ArrowDown');
  assert.ok(await page.locator('.toc-item').last().evaluate(el => el === document.activeElement));
  assert.ok((await currentPrompt.textContent()).includes('第一条指令'));
  await page.keyboard.press('Alt+ArrowDown');
  assert.ok((await currentPrompt.textContent()).includes('末尾指令'));
  await page.waitForFunction(() => document.querySelector('#ev-6003.flash') &&
    Number(document.querySelector('.toc-progress')?.getAttribute('aria-valuenow')) >= 95);
  await page.locator('.detail-body').evaluate(body => { body.scrollTop = 0; });
  await page.waitForFunction(() => document.querySelector('.toc-item[aria-current="location"]')?.textContent.includes('第一条指令'));
  const jumpBottom = page.getByRole('button', { name: '回到底部', exact: true });
  await jumpBottom.click();
  await page.waitForFunction(() => {
    const body = document.querySelector('.detail-body');
    return body.scrollHeight - body.clientHeight - body.scrollTop <= 4
      && body.querySelector('#ev-6003');
  });
  await jumpBottom.waitFor({ state: 'hidden' });
  assert.ok(await page.locator('.event-group').count()<50);
  await page.locator('.detail-body').evaluate(body => { body.scrollTop = 0; });
  await jumpBottom.waitFor();
  assert.equal(await page.evaluate(()=>window.__PERF__.calls.filter(c=>c.cmd==='read_tool_payload').length),2);
  await page.evaluate(()=>window.__PERF__.release());
  await page.waitForFunction(()=>document.querySelector('.session-card-sub').textContent.includes('条消息'));
  assert.equal(await page.locator('.session-card .model-label').textContent(),'logged-codex-model');
  await page.getByRole('button',{name:'配置',exact:true}).click();
  await page.getByRole('button',{name:'会话',exact:true}).click();
  await page.getByText('result on demand',{exact:true}).waitFor();
  const readCounts = await page.evaluate(() => ['read_session', 'read_tool_payload']
    .map(command => window.__PERF__.calls.filter(call => call.cmd === command).length));
  assert.deepEqual(readCounts, [1, 2], 'returning to an open conversation reuses its detail and expanded tool results');
  await page.getByRole('searchbox').fill('唯一定位针');
  await page.locator('.session-search-snippet').waitFor();await page.locator('.session-card').click();
  const mark=page.locator('#ev-6003 mark'); await mark.waitFor();
  await page.waitForFunction(()=>{const m=document.querySelector('#ev-6003 mark')?.getBoundingClientRect();const b=document.querySelector('.detail-body')?.getBoundingClientRect();return m&&b&&m.top>=b.top&&m.bottom<=b.bottom;});
  await page.setViewportSize({width:960,height:700});
  const outline = page.getByRole('button', {name:'对话导览', exact:true});
  await outline.focus();
  await outline.press('Space');
  await page.locator('.toc-item').first().waitFor();
  assert.equal(await page.locator('.toc').getByRole('searchbox').count(), 0);
  const bodyWidth = await page.locator('.detail-body').evaluate(el => el.clientWidth);
  await page.keyboard.press('Escape');
  await page.locator('.toc').waitFor({state:'hidden'});
  assert.ok(await outline.evaluate(el => el === document.activeElement));
  assert.equal(await page.locator('.detail-body').evaluate(el => el.clientWidth), bodyWidth);
  await outline.click();
  await page.locator('.toc-item').first().click();
  await page.waitForFunction(() => document.activeElement?.id === 'ev-0');
  await outline.click();
  await page.locator('.toc-item').last().click();
  await page.waitForFunction(() => document.activeElement?.id === 'ev-6003');
  const lastMessage = page.locator('#ev-6003 .msg-text');
  const prefix = () => mark.evaluate(el => el.previousSibling?.textContent ?? '');
  const initialPrefix = await prefix();
  assert.match(initialPrefix, /^…/);
  await lastMessage.getByRole('button',{name:'显示更多',exact:true}).click();
  assert.equal(await prefix(), initialPrefix, 'expanding after a late search match keeps the starting offset');
  assert.match(await lastMessage.textContent(), /结尾$/);
  await lastMessage.getByRole('button', { name: '显示前文', exact: true }).click();
  assert.match(await prefix(), /^末尾指令/);
  assert.deepEqual(errors,[]);
  console.log(`PASS: historical model in detail and refined card; bounded DOM, deferred tools, persistent expansion, background counts, TOC, resize and long-text search (${engine})`);
} finally {await browser.close();}
