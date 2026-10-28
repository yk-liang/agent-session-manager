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
      {type:'assistant',uuid:'tool',message:{content:[{type:'tool_use',id:'call',name:'shell',deferred:'input'}]}},
      {type:'user',uuid:'result',message:{content:[{type:'tool_result',tool_use_id:'call',deferred:'output'}]}},
      ...Array.from({length:6000},(_,i)=>({type:'assistant',uuid:`a-${i}`,message:{content:`消息 ${i}\n`+'正文示例\n'.repeat(10)}})),
      {type:'user',uuid:'last',message:{content:'末尾指令 '+ '长文本'.repeat(3000)+' 唯一定位针 '+ '结尾'.repeat(2000)}}];
    const state={calls:[],refining:false,refined:false,release:null};window.__PERF__=state;
    const catalog=statsPending=>({sessions:[{...meta,model:statsPending?null:meta.model,statsPending}],projects:[{dirName:'p',displayPath:'/tmp/long',sessionCount:1,lastActivity:null}],warnings:[]});
    window.__TAURI_INTERNALS__={transformCallback:cb=>cb,invoke:async(cmd,args={})=>{
      state.calls.push({cmd,args});
      if(cmd==='list_agents')return [{id:'codex',displayName:'Codex',installed:true,supportsFastCatalog:true,deletePolicy:'trash'}];
      if(cmd==='catalog')return catalog(!state.refined);
      if(cmd==='refine_catalog'){state.refining=true;await new Promise(resolve=>state.release=resolve);state.refined=true;return catalog(false);}
      if(cmd==='read_session')return {meta,events};
      if(cmd==='read_tool_payload')return args.token==='input'?{type:'tool_use',id:'call',name:'shell',input:{cmd:'loaded on demand'}}:{type:'tool_result',tool_use_id:'call',content:'result on demand',is_error:true};
      if(cmd==='search_conversations')return {hits:[{sessionId:'long',projectDir:'p',role:'user',eventIndex:events.length-1,eventId:'last',snippet:'唯一定位针'}],skipped:0,cancelled:false};
      return null;
    }};
  });
  await page.goto(baseURL);
  await page.locator('.session-card').waitFor();
  await page.waitForFunction(()=>window.__PERF__.refining);
  assert.ok(!(await page.locator('.session-card-sub').textContent()).includes('条对话'));
  assert.equal(await page.locator('.session-card .model-label').count(),0);
  const start=Date.now();await page.locator('.session-card').click();
  await page.locator('.event-group').first().waitFor();
  console.log(`PASS: first detail render ${Date.now()-start} ms with 6004 events (${engine})`);
  assert.ok(await page.locator('.event-group').count()<50);
  await page.getByRole('button',{name:'更多信息',exact:true}).click();
  assert.equal(await page.locator('.session-info-popover .model-label').textContent(),'logged-codex-model');
  await page.getByRole('button',{name:'更多信息',exact:true}).click();
  assert.equal(await page.evaluate(()=>window.__PERF__.calls.filter(c=>c.cmd==='read_tool_payload').length),0);
  await page.getByRole('button',{name:/工具调用 · shell/}).click();
  await page.getByText('result on demand',{exact:true}).waitFor();
  assert.equal(await page.evaluate(()=>window.__PERF__.calls.filter(c=>c.cmd==='read_tool_payload').length),2);
  await page.locator('.toc-item').last().click();
  await page.locator('#ev-6003').waitFor();
  assert.ok(await page.locator('.event-group').count()<50);
  await page.locator('.toc-item').first().click();
  await page.getByText('result on demand',{exact:true}).waitFor();
  assert.equal(await page.evaluate(()=>window.__PERF__.calls.filter(c=>c.cmd==='read_tool_payload').length),2);
  await page.evaluate(()=>window.__PERF__.release());
  await page.waitForFunction(()=>document.querySelector('.session-card-sub').textContent.includes('条对话'));
  assert.equal(await page.locator('.session-card .model-label').textContent(),'logged-codex-model');
  await page.getByRole('searchbox').fill('唯一定位针');
  await page.locator('.session-search-snippet').waitFor();await page.locator('.session-card').click();
  const mark=page.locator('#ev-6003 mark'); await mark.waitFor();
  await page.waitForFunction(()=>{const m=document.querySelector('#ev-6003 mark')?.getBoundingClientRect();const b=document.querySelector('.detail-body')?.getBoundingClientRect();return m&&b&&m.top>=b.top&&m.bottom<=b.bottom;});
  await page.setViewportSize({width:1100,height:700});
  await page.locator('.toc-item').first().click();await page.locator('#ev-0').waitFor();
  await page.locator('.toc-item').last().click();await page.locator('#ev-6003').waitFor();
  await page.getByRole('button',{name:'展开更多',exact:true}).click();
  assert.ok((await page.locator('#ev-6003').textContent()).includes('结尾'));
  assert.deepEqual(errors,[]);
  console.log(`PASS: historical model in detail and refined card; bounded DOM, deferred tools, persistent expansion, background counts, TOC, resize and long-text search (${engine})`);
} finally {await browser.close();}
