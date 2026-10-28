import { engine, baseURL, artifactDir } from "../helpers/browser.mjs";
import { chromium, webkit } from "playwright";
import assert from "node:assert/strict";
import { mkdirSync } from "node:fs";
import path from "node:path";


const browser=engine === "webkit" ? await webkit.launch({headless:true}) : await chromium.launch({channel:"chrome",headless:true});
const page=await browser.newPage({viewport:{width:1280,height:800}});
const errors=[];
page.on("pageerror",error=>errors.push(error.message));
const artifacts=artifactDir;
mkdirSync(artifacts,{recursive:true});
let checks=0;
const pass=label=>console.log(`PASS ${++checks}: ${label}`);
try {
  await page.addInitScript(()=>{
    const meta=(id,dir,title,cwd)=>({sessionId:id,projectDir:dir,title,cwd,model:null,firstTs:null,lastTs:null,
      realUserCount:1,assistantTextCount:1,userCount:1,assistantCount:1,sizeBytes:1000,hasTranscript:true});
    const rows=[meta("a","dir-a","未归属甲","/Users/demo/Documents/Codex/2026-10-16/task-a"),
      meta("b","dir-b","未归属乙","/tmp/task-b"),meta("named","dir-a","同目录有项目会话","/Users/demo/Documents/Codex/2026-10-16/task-a"),
      meta("manual","manual","手选目录会话","/Users/demo/Documents/Codex/manual")];
    const state={rows,ids:["a","b"],calls:[],deleted:[]};window.__GROUP_TEST__=state;
    window.__TAURI_INTERNALS__={transformCallback:cb=>cb,invoke:async(command,args={})=>{
      state.calls.push({command,args:structuredClone(args)});
      if(command==="list_agents")return [{id:"codex",displayName:"Codex",installed:true,deletePolicy:"trash",supportsFork:false},
        {id:"qoder",displayName:"Qoder CLI",installed:true,deletePolicy:"trash",supportsFork:false}];
      if(command==="catalog"){
        const sessions=args.agent==="codex"?state.rows:[meta("a","qoder-dir","另一 Agent 会话","/tmp/qoder")];
        const projects=[...new Set(sessions.map(s=>s.projectDir))].map(dir=>({dirName:dir,displayPath:sessions.find(s=>s.projectDir===dir).cwd,
          sessionCount:sessions.filter(s=>s.projectDir===dir).length,lastActivity:null}));
        return {sessions,projects,warnings:[],...(args.agent==="codex"?{projectlessSessionIds:state.ids}: {})};
      }
      if(command==="read_session"){
        const m=state.rows.find(s=>s.sessionId===args.sessionId&&s.projectDir===args.projectDir);
        if(!m)throw Error("Operation used a virtual group instead of the source project");
        return {meta:m,events:[{type:"user",uuid:"u-"+m.sessionId,message:{content:"真实目录仍用于恢复与会话操作"}},
          {type:"assistant",uuid:"hit-"+m.sessionId,message:{content:m.sessionId==="b"?"正文中的归组针，仅属于乙。":"普通回答"}}]};
      }
      if(command==="search_conversations")return {hits:args.query==="归组针"&&args.targets.some(t=>t.sessionId==="b")?
        [{sessionId:"b",projectDir:"dir-b",eventIndex:1,eventId:"hit-b",role:"assistant",snippet:"正文中的归组针，仅属于乙。"}]:[],skipped:0,cancelled:false};
      if(command==="cancel_conversation_search")return null;
      if(command==="delete_session"){
        if(!state.rows.some(s=>s.sessionId===args.sessionId&&s.projectDir===args.projectDir))throw Error("Wrong deletion identity");
        state.deleted.push(structuredClone(args));state.rows=state.rows.filter(s=>!(s.sessionId===args.sessionId&&s.projectDir===args.projectDir));return null;
      }
      throw Error(`Unexpected command: ${command}`);
    }};
  });
  await page.goto(baseURL);
  await page.locator(".session-card").first().waitFor();
  const group=page.locator(".sidebar-project-list .project-item",{hasText:"未归属项目"});
  assert.equal(await page.locator(".sidebar-project-list .project-item").count(),3);
  assert.ok((await group.textContent()).includes("2 个会话"));
  assert.ok((await page.getByRole("button",{name:/^全部会话/}).textContent()).includes("4 个会话"));
  assert.equal(await page.locator(".session-card .session-project-path").count(),2);
  assert.equal(await page.locator(".session-card",{hasText:"未归属乙"}).getAttribute("title"),"/tmp/task-b");
  pass("explicit projectless IDs form one group; source directories stay hidden on those cards only");

  await group.click();
  assert.equal(await page.locator(".session-card").count(),2);
  assert.equal(await page.locator(".session-card",{hasText:"同目录有项目会话"}).count(),0);
  await page.locator(".session-card",{hasText:"未归属甲"}).click();
  await page.getByRole("button",{name:"更多信息",exact:true}).click();
  assert.ok((await page.getByRole("group",{name:"会话信息"}).textContent()).includes("/Users/demo/Documents/Codex/2026-10-16/task-a"));
  await page.keyboard.press("Escape");
  await page.screenshot({path:path.join(artifacts,`project-groups-${engine}.png`)});
  pass("projectless selection excludes assigned sessions sharing a directory; details keep the full real cwd");

  await page.getByRole("searchbox").fill("归组针");
  await page.locator(".session-search-snippet").waitFor();
  const call=await page.evaluate(()=>window.__GROUP_TEST__.calls.filter(c=>c.command==="search_conversations").at(-1));
  assert.deepEqual(call.args.targets.map(t=>[t.projectDir,t.sessionId]).sort(),[["dir-a","a"],["dir-b","b"]]);
  await page.locator(".session-card").click();
  await page.locator(".search-target mark").waitFor();
  assert.equal(await page.locator(".search-target mark").textContent(),"归组针");
  pass("search targets retain original project identities across projectless directories and highlight the right message");

  await page.getByRole("searchbox").fill("");
  await page.evaluate(()=>window.__GROUP_TEST__.ids=[]);
  await page.evaluate(() => { window.dispatchEvent(new Event("blur")); window.dispatchEvent(new Event("focus")); });
  await page.waitForFunction(()=>document.querySelectorAll(".session-card").length===4);
  assert.equal(await group.count(),0);
  assert.equal(await page.locator(".sidebar-projects-header .project-item.active").count(),1);
  await page.evaluate(()=>window.__GROUP_TEST__.ids=["a","b"]);
  await page.evaluate(() => { window.dispatchEvent(new Event("blur")); window.dispatchEvent(new Event("focus")); });
  await group.waitFor();await group.click();
  assert.equal(await page.locator(".session-card").count(),2);
  pass("membership refresh restores directory grouping and clears a vanished virtual selection");

  await page.getByRole("button",{name:"批量管理",exact:true}).click();
  await page.getByRole("button",{name:"全选当前结果",exact:true}).click();
  await page.getByRole("button",{name:"删除所选",exact:true}).click();
  await page.getByRole("dialog").getByRole("button",{name:"移入回收站（2）",exact:true}).click();
  await page.waitForFunction(()=>window.__GROUP_TEST__.deleted.length===2);
  const deleted=await page.evaluate(()=>window.__GROUP_TEST__.deleted);
  assert.deepEqual(deleted.map(d=>[d.projectDir,d.sessionId]).sort(),[["dir-a","a"],["dir-b","b"]]);
  await page.waitForFunction(()=>document.querySelectorAll(".session-card").length===2);
  assert.equal(await page.locator(".session-card",{hasText:"同目录有项目会话"}).count(),1);
  pass("batch actions use real source projects and leave assigned sessions untouched");

  await page.getByRole("combobox", { name: "当前 Agent", exact: true }).selectOption({ label: "Qoder CLI" });
  await page.locator(".session-card",{hasText:"另一 Agent 会话"}).waitFor();
  assert.equal(await group.count(),0);
  assert.equal(await page.locator(".session-project-path").count(),1);
  assert.deepEqual(errors,[]);
  pass("agent switching cannot leak projectless membership into another data source");
  console.log(`PASS: ${checks} project grouping checks (${engine})`);
} catch(error) {
  await page.screenshot({path:path.join(artifacts,`project-groups-failure-${engine}.png`)}).catch(()=>{});
  console.error(error);process.exitCode=1;
} finally {await browser.close();}
