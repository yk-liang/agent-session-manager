import { engine, baseURL, artifactDir } from "../helpers/browser.mjs";
import {chromium,webkit} from "playwright";
import assert from "node:assert/strict";
import {mkdirSync} from "node:fs";
import path from "node:path";

const browser=engine==="webkit"?await webkit.launch({headless:true}):await chromium.launch({channel:"chrome",headless:true});
const page=await browser.newPage({viewport:{width:1280,height:800}});
const errors=[];page.on("pageerror",e=>errors.push(e.message));
const artifacts=artifactDir;mkdirSync(artifacts,{recursive:true});
let checks=0;const pass=s=>console.log(`PASS ${++checks}: ${s}`);
try{
  await page.addInitScript(()=>{
    const meta=(id,dir,title,cwd)=>({sessionId:id,projectDir:dir,title,cwd,model:null,firstTs:null,lastTs:null,userCount:1,assistantCount:1,realUserCount:1,assistantTextCount:1,sizeBytes:1000,hasTranscript:true});
    const state={rows:[meta("shared","run-a","诊断会话甲","/Users/demo/work/real-project/.local/diag-a"),meta("b","run-b","诊断会话乙","/Users/demo/work/real-project/poc/.local/diag-b"),meta("shared","auto","独立工作区","/Users/demo/Documents/Qoder/2026-10-16/1234abcd"),meta("m","manual","普通目录","/tmp/manual")],calls:[],deleted:[]};
    window.__QODER_GROUP__=state;
    window.__TAURI_INTERNALS__={transformCallback:cb=>cb,invoke:async(command,args={})=>{
      state.calls.push({command,args:structuredClone(args)});
      if(command==="list_agents")return ["qoder","codex"].map(id=>({id,displayName:id==="qoder"?"Qoder CLI":"Codex",installed:true,supportsFork:false,deletePolicy:"trash"}));
      if(command==="catalog"){
        const sessions=args.agent==="qoder"?state.rows:[meta("shared","codex-src","Codex 会话","/tmp/codex")];
        return {sessions,projects:sessions.map(s=>({dirName:s.projectDir,displayPath:s.cwd,sessionCount:1,lastActivity:null})),warnings:[],...(args.agent==="qoder"?{sessionGroupHints:state.rows.filter(s=>s.projectDir!=="manual").map(s=>({projectDir:s.projectDir,sessionId:s.sessionId,root:s.projectDir==="auto"?null:"/Users/demo/work/real-project"}))}:{projectlessSessionIds:["shared"]})};
      }
      if(command==="read_session"){
        const meta=state.rows.find(s=>s.sessionId===args.sessionId&&s.projectDir===args.projectDir);
        if(!meta)throw Error("Source identity was replaced by a group key");
        return {meta,events:[{type:"user",uuid:"u",message:{content:"user question"}},{type:"assistant",uuid:"a",message:{content:"Qoder归组针命中正文"}}]};
      }
      if(command==="search_conversations")return {hits:args.query==="Qoder归组针"&&args.targets.some(t=>t.projectDir==="run-b")?[{sessionId:"b",projectDir:"run-b",eventIndex:1,eventId:"a",role:"assistant",snippet:"Qoder归组针命中正文"}]:[],skipped:0,cancelled:false};
      if(command==="cancel_conversation_search")return null;
      if(command==="delete_session"){
        if(!state.rows.some(s=>s.sessionId===args.sessionId&&s.projectDir===args.projectDir))throw Error("Wrong delete identity");
        state.deleted.push(structuredClone(args));state.rows=state.rows.filter(s=>!(s.sessionId===args.sessionId&&s.projectDir===args.projectDir));return null;
      }
      throw Error(`Unexpected command: ${command}`);
    }};
  });
  await page.goto(baseURL);await page.locator(".session-card").first().waitFor();
  assert.equal(await page.locator(".sidebar-project-list .project-item").count(),3);
  const root=page.locator(".sidebar-project-list .project-item",{hasText:"~/work/real-project"});
  const unassigned=page.locator(".sidebar-project-list .project-item",{hasText:"未归属项目"});
  assert.ok((await root.textContent()).includes("2 个会话"));
  assert.ok((await unassigned.textContent()).includes("1 个会话"));
  assert.equal(await page.locator(".session-card",{hasText:"诊断会话甲"}).locator(".session-project-path").count(),1);
  assert.equal(await page.locator(".session-card",{hasText:"诊断会话甲"}).locator(".session-project-path").textContent(),"~/work/real-project");
  assert.equal(await page.locator(".session-card",{hasText:"诊断会话甲"}).locator(".session-project-path").getAttribute("title"),"/Users/demo/work/real-project/.local/diag-a");
  assert.equal(await page.locator(".session-card",{hasText:"独立工作区"}).locator(".session-project-path").count(),0);
  pass("run directories merge under the real root and same-ID automatic sessions stay separate");
  await root.click();assert.equal(await page.locator(".session-card").count(),2);
  await page.locator(".session-card",{hasText:"诊断会话甲"}).click();
  await page.getByRole("button",{name:"更多信息",exact:true}).click();
  assert.ok((await page.getByRole("group",{name:"会话信息"}).textContent()).includes("/Users/demo/work/real-project/.local/diag-a"));
  await page.keyboard.press("Escape");
  await page.screenshot({path:path.join(artifacts,`qoder-groups-${engine}.png`)});
  pass("root filtering and details keep each run's real working directory");
  await page.getByRole("searchbox").fill("Qoder归组针");await page.locator(".session-search-snippet").waitFor();
  const request=await page.evaluate(()=>window.__QODER_GROUP__.calls.filter(c=>c.command==="search_conversations").at(-1));
  assert.deepEqual(request.args.targets.map(t=>[t.projectDir,t.sessionId]).sort(),[["run-a","shared"],["run-b","b"]]);
  await page.locator(".session-card").click();await page.locator(".search-target mark").waitFor();
  pass("project search targets source directories and locates the matched message");
  await page.getByRole("searchbox").fill("");await page.getByRole("button",{name:"批量管理",exact:true}).click();
  await page.getByRole("button",{name:"全选当前结果",exact:true}).click();await page.getByRole("button",{name:"删除所选",exact:true}).click();
  await page.getByRole("dialog").getByRole("button",{name:"移入回收站（2）",exact:true}).click();
  await page.waitForFunction(()=>window.__QODER_GROUP__.deleted.length===2);
  assert.deepEqual((await page.evaluate(()=>window.__QODER_GROUP__.deleted)).map(t=>[t.projectDir,t.sessionId]).sort(),[["run-a","shared"],["run-b","b"]]);
  await page.waitForFunction(()=>document.querySelectorAll(".session-card").length===2);
  assert.equal(await page.locator(".session-card",{hasText:"独立工作区"}).count(),1);
  pass("batch operations preserve same-ID sessions belonging to other source directories");
  await page.locator(".sidebar").getByRole("button",{name:"Codex",exact:true}).click();
  await page.locator(".session-card",{hasText:"Codex 会话"}).waitFor();
  assert.equal(await root.count(),0);assert.equal(await unassigned.count(),1);
  assert.deepEqual(errors,[]);pass("agent switching clears Qoder root hints while retaining Codex classification");
  console.log(`PASS: ${checks} Qoder grouping checks (${engine})`);
}catch(error){await page.screenshot({path:path.join(artifacts,`qoder-groups-failure-${engine}.png`)}).catch(()=>{});console.error(error);process.exitCode=1;}
finally{await browser.close();}
