import { engine, baseURL, artifactDir } from "../helpers/browser.mjs";
// 同一条真实 OpenCode 会话复现：样式切换后已测量行应继续紧邻，不可停留在估算高度。
import { webkit, chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const dir=mkdtempSync(path.join(tmpdir(),'asm-layout-repro-'));
const exported=path.join(dir,'detail.json');
const artifacts=artifactDir;mkdirSync(artifacts,{recursive:true});
let browser,page;
const gaps=()=>Array.from(document.querySelectorAll('.event-group')).flatMap((row,i,rows)=>{
  if(i===0)return[];const previous=rows[i-1];
  if(Number(row.dataset.index)!==Number(previous.dataset.index)+1)return[];
  return [{previous:previous.id,next:row.id,gap:Math.round((row.getBoundingClientRect().top-previous.getBoundingClientRect().bottom)*10)/10}];
});
const compact=async()=>{
  await page.waitForTimeout(100);
  const values=await page.evaluate(gaps);
  assert.ok(values.length>=2,'需要多个相邻行验证高度');
  const wrong=values.filter(v=>v.gap<9||v.gap>11);
  assert.deepEqual(wrong,[],'相邻消息必须保持 10px 间距');
};
try{
  let detail;
  if(process.env.ASM_LAYOUT_USE_REAL==='1') {
    await promisify(execFile)('cargo',['run','--offline','--locked','--example','opencode_perf'],{cwd:path.resolve('src-tauri'),env:{...process.env,ASM_OPENCODE_SESSION:'ses_f36bdb6b8ffe8aaxELwqmbD8tU',ASM_OPENCODE_VIEW_EXPORT:exported},timeout:180000});
    detail=JSON.parse(readFileSync(exported,'utf8'));
  } else {
    const events=[{type:'user',uuid:'u1',message:{content:'# 用户请求\n\n'+('- 条件 **约束**\n').repeat(160)}}];
    for(let i=0;i<30;i++)events.push(
      {type:'assistant',uuid:`think-${i}`,message:{content:[{type:'thinking',thinking:'分析当前步骤'}]}},
      {type:'assistant',uuid:`text-${i}`,message:{content:`已完成当前检查 ${i}。`}},
      {type:'assistant',uuid:`call-${i}`,message:{content:[{type:'tool_use',name:'read',id:`call-${i}`,deferred:`fixture-${i}:input`}]}},
      {type:'user',uuid:`result-${i}`,message:{content:[{type:'tool_result',tool_use_id:`call-${i}`,deferred:`fixture-${i}:result`}]}}
    );
    detail={meta:{sessionId:'fixture',projectDir:'p',cwd:'/tmp/layout',title:'折叠行高度回归',model:null,firstTs:null,lastTs:null,userCount:1,assistantCount:30,realUserCount:1,assistantTextCount:30,sizeBytes:10000,hasTranscript:true},events};
  }
  browser=engine==='webkit'?await webkit.launch({headless:true}):await chromium.launch({channel:'chrome',headless:true});
  page=await browser.newPage({viewport:{width:1500,height:940}});
  await page.addInitScript(detail=>{
    window.__LAYOUT_CALLS__=[];
    window.__TAURI_INTERNALS__={transformCallback:cb=>cb,invoke:async(cmd,args={})=>{
      window.__LAYOUT_CALLS__.push({cmd,args});
      if(cmd==='list_agents')return[{id:'opencode',displayName:'OpenCode',installed:true,supportsFork:true,deletePolicy:'permanent'}];
      if(cmd==='catalog')return{sessions:[detail.meta],projects:[{dirName:detail.meta.projectDir,displayPath:detail.meta.cwd,sessionCount:1,lastActivity:null}],warnings:[]};
      if(cmd==='read_session')return detail;
      if(cmd==='read_tool_payload')return args.token.endsWith(':input')?{type:'tool_use',input:{command:'geometry-only fixture'}}:{type:'tool_result',content:'## 结果\n\n'+'输出正文\n'.repeat(25)};
      return null;
    }};
  },detail);
  await page.goto(baseURL);await page.locator('.session-card').click();
  await page.locator('.event-group').first().waitFor();await compact();
  const markdown=page.getByRole('checkbox',{name:'Markdown',exact:true});await markdown.check();
  await page.locator('.markdown-body').first().waitFor();await page.waitForFunction(()=>!document.querySelector('[data-markdown-pending]'));
  await compact();
  console.log(`PASS: OpenCode-shaped history remains compact after Markdown toggling (${engine})`);
  for(let round=0;round<3;round++){
    await page.locator('.detail-body').evaluate(el=>el.scrollTo(0,el.scrollTop+420));await compact();
    await markdown.uncheck();await compact();await markdown.check();await page.waitForFunction(()=>!document.querySelector('[data-markdown-pending]'));await compact();
  }
  const tool=page.locator('.collapsible.tool .collapsible-head').first();
  await tool.click();await page.locator('.collapsible-body').first().waitFor();await compact();
  await tool.click();await compact();
  for(const name of ['工具调用','思考过程']){await page.getByRole('checkbox',{name,exact:true}).uncheck();await compact();await page.getByRole('checkbox',{name,exact:true}).check();await compact();}
  await page.setViewportSize({width:1100,height:760});await compact();
  console.log(`PASS: scroll, repeated toggling, tool expansion/filter changes and resize have no stale gaps (${engine})`);
  const style=()=>Array.from(document.querySelectorAll('.view-toggles input')).map(input=>{const css=getComputedStyle(input);return{appearance:css.getPropertyValue('appearance')||css.getPropertyValue('-webkit-appearance'),background:css.backgroundColor,border:css.borderColor};});
  const active=await page.evaluate(style);assert.ok(active.every(s=>s.appearance==='none'),'复选框不能使用会随系统焦点变化的原生外观');
  await page.evaluate(()=>window.dispatchEvent(new Event('blur')));assert.deepEqual(await page.evaluate(style),active);
  await page.evaluate(()=>window.dispatchEvent(new Event('focus')));assert.deepEqual(await page.evaluate(style),active);
  for(const name of ['工具调用','思考过程','Markdown']){
    const box=page.getByRole('checkbox',{name,exact:true});await box.focus();const before=await box.isChecked();await box.press('Space');assert.equal(await box.isChecked(),!before);await box.press('Space');assert.equal(await box.isChecked(),before);
  }
  console.log(`PASS: explicit checkbox colors survive focus events and retain Space-key/accessibility behavior (${engine})`);
}catch(error){if(page){console.error('Row gaps:',JSON.stringify(await page.evaluate(gaps).catch(()=>[])));await page.screenshot({path:path.join(artifacts,`detail-layout-failure-${engine}.png`)}).catch(()=>{});}throw error;}
finally{await browser?.close();rmSync(dir,{recursive:true,force:true});}
