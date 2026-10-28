import { engine, baseURL, artifactDir } from "../helpers/browser.mjs";
import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdirSync } from 'node:fs';
import path from 'node:path';


const browser = engine === 'webkit' ? await webkit.launch({headless:true}) : await chromium.launch({channel:'chrome',headless:true});
const page = await browser.newPage({viewport:{width:1280,height:800}});
const errors=[];
page.on('pageerror', e=>errors.push(e.message));
const artifacts=artifactDir;
mkdirSync(artifacts,{recursive:true});
let checks=0;
const pass=label=>console.log(`PASS ${++checks}: ${label}`);

try {
  await page.addInitScript(() => {
    const meta=(id,project,title)=>({sessionId:id,projectDir:project,title,cwd:`/tmp/${project.slice(1)}`,model:'auto',firstTs:null,lastTs:null,realUserCount:1,assistantTextCount:1,userCount:1,assistantCount:1,sizeBytes:2048,hasTranscript:true});
    const a=meta('aaaa','-one','普通标题甲'),b=meta('bbbb','-two','普通标题乙'),c=meta('cccc','-one','目标针·标题匹配'),d=meta('dddd','-codex','另一 Agent 的记录');
    const filler=Array.from({length:80},(_,i)=>({type:'assistant',uuid:`filler-${i}`,message:{content:`前面的消息 ${i}\n`+'用于滚动定位的正文\n'.repeat(8)}}));
    const messages={
      aaaa:[...filler,{type:'user',uuid:'hit-aaaa',message:{content:'这里只在用户正文里出现 目标针 😀，标题没有这个词。'}},{type:'assistant',uuid:'hidden',message:{content:[{type:'thinking',thinking:'隐藏思考词'},{type:'tool_use',id:'tool',input:{text:'工具日志词'}}]}}],
      bbbb:[{type:'assistant',uuid:'hit-bbbb',message:{content:[{type:'text',text:'助手正文包含 目标针，也有 新查询。'}]}}],
      cccc:[{type:'user',uuid:'no-hit',message:{content:'只有标题匹配，正文不匹配。'}}],
      dddd:[{type:'user',uuid:'another-hit',message:{content:'目标针是另一个问题。'}},{type:'assistant',message:{content:'这是另一 Agent 的目标针正文。'}}],
    };
    const agents=[{id:'qoder',displayName:'Qoder CLI',installed:true,supportsFork:true,dataDir:'/tmp/qoder',deletePolicy:'trash'},{id:'codex',displayName:'Codex',installed:true,supportsFork:false,dataDir:'/tmp/codex',deletePolicy:'trash'}];
    const records={qoder:[a,b,c],codex:[d]};
    const state={calls:[],hold:false,pending:[],fail:false,moveMessage:false};window.__SEARCH_TEST__=state;
    window.__TAURI_INTERNALS__={transformCallback:cb=>cb,invoke:async(command,args={})=>{
      state.calls.push({command,args:structuredClone(args)});
      if(command==='list_agents')return agents;
      if(command==='cancel_conversation_search')return null;
      if(command==='catalog'){
        const sessions=records[args.agent];
        return {sessions,projects:[...new Set(sessions.map(s=>s.projectDir))].map(project=>({dirName:project,displayPath:`/tmp/${project.slice(1)}`,sessionCount:sessions.filter(s=>s.projectDir===project).length,lastActivity:null})),warnings:[]};
      }
      if(command==='read_session'){
        const meta=records[args.agent].find(s=>s.sessionId===args.sessionId&&s.projectDir===args.projectDir);
        if(!meta)throw Error('Wrong project or agent');
        const events=structuredClone(messages[meta.sessionId]);
        if(state.moveMessage&&meta.sessionId==='aaaa')events.unshift({type:'user',uuid:'inserted',message:{content:'后来插入的消息'}});
        return {meta,events};
      }
      if(command==='search_conversations'){
        const held=state.hold;
        if(held)await new Promise(resolve=>state.pending.push(resolve));
        if(state.fail)throw Error('Mock search failed');
        const ids=args.query==='目标针'?(args.agent==='codex'?['dddd']:['aaaa','bbbb']):args.query==='旧查询'?['aaaa']:args.query==='新查询'?['bbbb']:[];
        const hits=ids.flatMap(id=>{
          const meta=records[args.agent].find(s=>s.sessionId===id);
          if(!meta||!args.targets.some(t=>t.sessionId===id&&t.projectDir===meta.projectDir))return[];
          return[{sessionId:id,projectDir:meta.projectDir,eventIndex:id==='aaaa'?12:0,eventId:id==='dddd'?null:`hit-${id}`,role:id==='aaaa'?'user':'assistant',snippet:id==='aaaa'?'…用户正文里出现 目标针 😀，标题没有这个词。':id==='bbbb'?`助手正文包含 ${args.query}。`:'另一 Agent 的目标针正文。'}];
        });
        return {hits,skipped:args.query==='部分失败'?1:0,cancelled:false};
      }
      throw Error(`Unexpected command: ${command}`);
    }};
  });
  await page.goto(baseURL);
  const search=page.getByRole('searchbox',{name:'搜索会话或对话内容',exact:true});
  await page.locator('.session-card').first().waitFor();
  await search.fill('目标针');
  await page.locator('.session-search-snippet').first().waitFor();
  assert.equal(await page.locator('.session-card').count(),3);
  assert.equal(await page.locator('.session-search-snippet').count(),2);
  assert.equal(await page.locator('.session-search-snippet mark').first().textContent(),'目标针');
  assert.equal(await page.locator('.search-status').count(),0);
  pass('body-only matches join metadata matches; snippets highlight the literal query');

  await page.evaluate(()=>window.__SEARCH_TEST__.moveMessage=true);
  await page.locator('.session-card',{hasText:'普通标题甲'}).click();
  const hit=page.locator('.detail-body .search-target');
  await hit.waitFor();
  assert.equal(await hit.getAttribute('id'),'ev-81');
  assert.equal(await hit.locator('mark').textContent(),'目标针');
  const box=await hit.locator('mark').boundingBox();
  const body=await page.locator('.detail-body').boundingBox();
  assert.ok(box.y>=body.y&&box.y+box.height<=body.y+body.height,'matched message should be visible');
  assert.ok(await page.locator('.detail-body').evaluate(el=>el.scrollTop)>500);
  await page.screenshot({path:path.join(artifacts,`conversation-search-${engine}.png`)});
  pass('click finds the message by UUID after event positions change, scrolls and highlights');

  await page.locator('.sidebar-project-list .project-item',{hasText:'/tmp/one'}).click();
  await page.waitForFunction(()=>window.__SEARCH_TEST__.calls.filter(c=>c.command==='search_conversations').at(-1).args.targets.every(t=>t.projectDir==='-one'));
  await page.locator('.session-search-snippet').waitFor();
  assert.equal(await page.locator('.session-card').count(),2);
  assert.equal(await page.locator('.session-project-path').count(),0);
  assert.equal(await page.locator('.session-card',{hasText:'普通标题乙'}).count(),0);
  pass('project scope constrains backend targets and visible search results');

  await page.getByRole('button',{name:/^全部会话/}).click();
  await page.locator('.session-card',{hasText:'普通标题乙'}).waitFor();
  assert.ok(await page.locator('.session-project-path').count()>0);
  await page.evaluate(()=>window.__SEARCH_TEST__.hold=true);
  await search.fill('旧查询');
  await page.waitForFunction(()=>window.__SEARCH_TEST__.pending.length===1);
  await page.evaluate(()=>window.__SEARCH_TEST__.hold=false);
  await search.fill('新查询');
  await page.locator('.session-card',{hasText:'普通标题乙'}).waitFor();
  await page.evaluate(()=>window.__SEARCH_TEST__.pending.splice(0).forEach(resolve=>resolve()));
  assert.equal(await page.locator('.session-card',{hasText:'普通标题甲'}).count(),0);
  assert.equal(await page.locator('.session-card').count(),1);
  pass('late responses from an old query cannot reintroduce stale hits');

  await page.evaluate(()=>window.__SEARCH_TEST__.hold=true);
  await search.fill('目标针');
  await page.waitForFunction(()=>window.__SEARCH_TEST__.pending.length===1);
  await page.evaluate(()=>window.__SEARCH_TEST__.hold=false);
  await page.locator('.sidebar').getByRole('button',{name:'Codex',exact:true}).click();
  await page.locator('.session-card',{hasText:'另一 Agent 的记录'}).waitFor();
  await page.evaluate(()=>window.__SEARCH_TEST__.pending.splice(0).forEach(resolve=>resolve()));
  assert.equal(await page.locator('.session-card').count(),1);
  assert.ok((await page.locator('.session-search-snippet').textContent()).includes('另一 Agent'));
  await page.locator('.session-card').click();
  await page.locator('.detail-body .search-target mark').waitFor();
  assert.equal(await page.locator('.detail-body .search-target').getAttribute('id'),'ev-1');
  await page.getByRole('button',{name:'清除搜索',exact:true}).click();
  assert.equal(await search.inputValue(),'');
  assert.equal(await page.getByRole('button',{name:'清除搜索',exact:true}).count(),0);
  assert.equal(await page.locator('.detail-body mark').count(),0);
  pass('switching Agent isolates results and clearing the search removes highlighting');

  await search.fill('不存在的词');
  await page.getByText('没有匹配的会话',{exact:true}).waitFor();
  assert.equal(await page.locator('.session-card').count(),0);
  await search.fill('部分失败');
  await page.getByRole('status').getByText('1 个会话未能搜索',{exact:true}).waitFor();
  await page.evaluate(()=>window.__SEARCH_TEST__.fail=true);
  await search.fill('失败查询');
  await page.getByText('正文搜索失败，可刷新重试',{exact:true}).waitFor();
  await page.evaluate(()=>window.__SEARCH_TEST__.fail=false);
  await search.fill('目标针');
  await page.locator('.session-search-snippet').waitFor();
  assert.deepEqual(errors,[]);
  pass('no-match, partial failure and retry states remain usable without browser errors');
  console.log(`PASS: ${checks} conversation search checks (${engine})`);
} catch(error) {
  await page.screenshot({path:path.join(artifacts,`conversation-search-failure-${engine}.png`)}).catch(()=>{});
  console.error(error);process.exitCode=1;
} finally { await browser.close(); }
