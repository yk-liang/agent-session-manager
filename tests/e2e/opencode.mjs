import { engine, baseURL } from "../helpers/browser.mjs";
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

// UI 使用真实 SQLite 解析器生成的无敏感信息 fixture，工具内容也来自同一次转换。
const privateDir=mkdtempSync(path.join(tmpdir(),'asm-opencode-fixture-'));
const exported=path.join(privateDir,'fixture.json');

let browser;
try {
  const run=await promisify(execFile)('cargo',['test','--offline','--locked','export_ordered_fixture_for_browser_if_requested','--','--nocapture'],
    {cwd:path.resolve('src-tauri'),env:{...process.env,ASM_OPENCODE_EXPORT:exported},timeout:180000});
  assert.match(run.stdout,/1 passed/);
  const fixture=JSON.parse(readFileSync(exported,'utf8'));
  browser=engine==='webkit'?await webkit.launch({headless:true}):await chromium.launch({channel:'chrome',headless:true});
  const page=await browser.newPage({viewport:{width:1280,height:800}});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(({detail,payloads})=>{
    const calls=[];window.__OPENCODE__={calls};
    const meta=new URLSearchParams(location.search).has('no-size')?{...detail.meta,sizeBytes:undefined}:detail.meta;
    const archived={...meta,sessionId:'ses_archived',title:'Archived conversation',archived:true};
    window.__TAURI_INTERNALS__={transformCallback:cb=>cb,invoke:async(cmd,args={})=>{
      calls.push({cmd,args});
      if(cmd==='list_agents')return[{id:'opencode',displayName:'OpenCode',installed:true,supportsFork:true,supportsArchive:false,supportsArchivedSessions:true,deletePolicy:'permanent'}];
      if(cmd==='catalog'){
        const sessions=args.includeArchived?[meta,archived]:[meta];
        return{sessions,projects:[{dirName:meta.projectDir,displayPath:meta.cwd,sessionCount:sessions.length,lastActivity:null}],warnings:[]};
      }
      if(cmd==='read_session')return structuredClone(args.sessionId===archived.sessionId?{...detail,meta:archived}:detail);
      if(cmd==='read_tool_payload'){if(!payloads[args.token])throw Error('Unknown tool reference');return structuredClone(payloads[args.token]);}
      if(cmd==='search_conversations'){
        const index=detail.events.findLastIndex(e=>typeof e.message?.content==='string'&&e.message.content.includes(args.query));
        return{hits:index<0?[]:[{sessionId:detail.meta.sessionId,projectDir:detail.meta.projectDir,role:'assistant',eventIndex:index,eventId:detail.events[index].uuid,snippet:args.query}],skipped:0,cancelled:false};
      }
      return null;
    }};
  },fixture);
  await page.goto(baseURL);
  await page.locator('.session-card .model-label').waitFor();
  assert.equal(await page.locator('.session-card .model-label').textContent(),'fixture/model-b');
  assert.ok(fixture.detail.meta.sizeBytes > 0);
  assert.match(await page.locator('.session-card-size').textContent(), /^[1-9]\d*(?:\.\d+)? (?:B|KB|MB)$/);
  await page.locator('.session-card').click();
  await page.getByRole('button',{name:/^工具调用 read.*已完成$/}).waitFor();
  const ids=await page.locator('.event-group').evaluateAll(rows=>rows.map(el=>Number(el.id.slice(3))));
  assert.deepEqual(ids,[...ids].sort((a,b)=>a-b));
  assert.ok(ids.indexOf(1)<ids.indexOf(2)&&ids.indexOf(2)<ids.indexOf(3)&&ids.indexOf(3)<ids.indexOf(5));
  assert.ok((await page.locator('#ev-0').textContent()).includes('First request\ncontinued request'));
  assert.equal(await page.evaluate(()=>window.__OPENCODE__.calls.filter(c=>c.cmd==='read_tool_payload').length),0);
  assert.equal(await page.locator('#ev-1').getAttribute('title'),'模型：fixture/model-a');
  await page.getByRole('button',{name:'更多信息',exact:true}).click();
  const info=page.getByRole('group',{name:'会话信息',exact:true});
  assert.equal(await info.locator('dt').first().textContent(),'最近使用的模型');
  assert.equal(await info.locator('.model-label').first().textContent(),'fixture/model-b');
  assert.equal(await info.locator('.session-model-history .model-label').textContent(),'fixture/model-a');
  await page.getByRole('button',{name:'更多信息',exact:true}).click();
  console.log(`PASS: latest model badge, provider-scoped history and per-message model hints (${engine})`);
  console.log(`PASS: source part order, grouped user prompt and folded tools/reasoning (${engine})`);

  await page.getByRole('button',{name:/^工具调用 read.*已完成$/}).click();
  await page.getByText('Tool output',{exact:true}).waitFor();
  assert.ok((await page.locator('.collapsible.tool .code').first().textContent()).includes('/tmp/example'));
  assert.equal(await page.evaluate(()=>window.__OPENCODE__.calls.filter(c=>c.cmd==='read_tool_payload').length),2);
  await page.getByRole('button',{name:/思考过程.*Stored reasoning/}).click();
  await page.locator('.collapsible.thinking .collapsible-body').getByText('Stored reasoning',{exact:true}).waitFor();
  console.log(`PASS: input/output load only after expansion; stored reasoning expands (${engine})`);

  await page.locator('.detail-body').evaluate(el=>el.scrollTo(0,el.scrollHeight));
  await page.getByRole('button',{name:/^工具调用 bash.*失败$/}).click();
  await page.getByText('Exit code 1',{exact:true}).waitFor();
  assert.ok(await page.getByRole('button',{name:/^工具调用 queued.*等待中$/}).count());
  await page.getByRole('button',{name:/^工具调用 streaming.*进行中$/}).click();
  await page.getByText('partial output',{exact:true}).waitFor();
  assert.equal(await page.getByRole('button',{name:/^工具调用 streaming.*已完成$/}).count(),0);
  await page.getByRole('checkbox',{name:'工具调用',exact:true}).uncheck();
  assert.equal(await page.locator('.collapsible.tool').count(),0);
  await page.getByRole('checkbox',{name:'思考过程',exact:true}).uncheck();
  assert.equal(await page.locator('.collapsible.thinking').count(),0);
  assert.ok(await page.getByText('Last visible answer, final needle',{exact:true}).count());
  console.log(`PASS: errors, pending/running states and independent visibility toggles (${engine})`);

  await page.getByRole('searchbox').fill('final needle');await page.locator('.session-search-snippet').waitFor();
  await page.locator('.session-card').click();
  const last=fixture.detail.events.findLastIndex(e=>typeof e.message?.content==='string'&&e.message.content.includes('final needle'));
  const mark=page.locator(`#ev-${last}.search-target mark`);await mark.waitFor();
  await page.waitForFunction(index=>{const m=document.querySelector(`#ev-${index} mark`)?.getBoundingClientRect();const b=document.querySelector('.detail-body')?.getBoundingClientRect();return m&&b&&m.top>=b.top&&m.bottom<=b.bottom;},last);
  assert.equal(await mark.textContent(),'final needle');assert.deepEqual(errors,[]);
  console.log(`PASS: search locates the correct body after interleaved tool/thinking parts (${engine})`);
  await page.goto(new URL('?no-size', baseURL).href);
  await page.locator('.session-card-sub').waitFor();
  assert.match(await page.locator('.session-card-stats').textContent(), /^\d+ 条消息$/);
  console.log(`PASS: measured session sizes render and unavailable sizes are omitted (${engine})`);
  const archiveFilter=page.getByRole('checkbox',{name:'显示已归档',exact:true});
  const archivedCard=page.locator('.session-card.archived');
  assert.equal(await archivedCard.count(),0);
  await archiveFilter.check();
  await archivedCard.getByText('Archived conversation',{exact:true}).click();
  await page.locator('.detail-title').filter({hasText:/^Archived conversation$/}).waitFor();
  assert.equal(await page.getByRole('button',{name:/取消归档/}).count(),0);
  await archiveFilter.uncheck();
  await archivedCard.waitFor({state:'hidden'});
  console.log(`PASS: archived sessions can be viewed with the catalog filter (${engine})`);
} finally {await browser?.close();rmSync(privateDir,{recursive:true,force:true});}
