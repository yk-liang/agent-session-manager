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
    const standaloneOutput=['Exit code 2',...Array.from({length:30},(_,i)=>`output line ${i+1}`)].join('\n');
    const longPreview='超长消息正文 '.repeat(500);
    const markdown='# 汇总标题\n\n这是 **加粗重点** 和 *斜体*、~~删除文字~~、`inlineCode`。\n\n- 列表甲\n- 列表乙\n\n1. 第一步\n2. 第二步\n\n> 引用内容\n\n| 项目 | 状态 |\n| --- | --- |\n| Markdown | 完成 |\n\n- [x] 已完成事项\n- [ ] 待办事项\n\n```ts\nconst codeNeedle = "'+ 'x'.repeat(240)+'";\n```\n\n[参考资料](https://example.com/guide)';
    const messages={
      md:[user('u-md','## 用户标题\n\n- **用户重点**'),assistant('a-md',markdown)],
      plain:[user('u-plain','普通文本\n下一行    多个空格\n数量：2 * 3 = 6'),assistant('a-json','{"key":"**不要格式化**","items":["#不是标题"]}'),assistant('a-xml','<status>不是 Markdown</status>')],
      tools:[user('u-tools','查看日志'),{type:'assistant',uuid:'a-tools',message:{content:[{type:'thinking',thinking:'## 思考标题\n**保持原文**'},{type:'tool_use',id:'call',name:'fixture',deferred:'tool-input',description:'查看运行日志'}, {type:'text',text:'样例助手正文'}]}},{type:'user',uuid:'r-tools',message:{content:[{type:'tool_result',tool_use_id:'call',deferred:'tool-output'}]}},
        {type:'assistant',message:{content:[{type:'tool_use',id:'standalone',name:'Bash',input:{command:'git status --short'},ops:[{kind:'shell',intent:'git',head:'git status --short',command:'git status --short'}]}]}},
        {type:'user',message:{content:[{type:'tool_result',tool_use_id:'standalone',is_error:true,content:standaloneOutput}]}},
        {type:'assistant',message:{content:[{type:'tool_use',id:'antigravity-cmd',name:'run_command',input:{CommandLine:'git diff --stat && ls -la',WaitMsBeforeAsync:5000,toolSummary:'检查仓库变更'},ops:[{kind:'shell',intent:'git_diff',head:'git diff --stat',more:1,command:'git diff --stat && ls -la',description:'检查仓库变更'}]}]}},
        {type:'user',message:{content:[{type:'tool_result',tool_use_id:'antigravity-cmd',status:'completed',content:'src/sessionContent.ts | 10 +++'}]}},
        assistant('a-preview',longPreview),user('u-preview',longPreview)],
      cards:[{type:'assistant',message:{content:[
        {type:'tool_use',id:'skill',name:'Skill',input:{skill:'fixture',args:'--brief'},ops:[{kind:'skill',name:'fixture',args:'--brief',body:'# 技能标题\n**技能正文**'}]},
        {type:'tool_use',id:'ask',name:'AskUserQuestion',deferred:'ask-input',ops:[{kind:'ask',questions:[{question:'使用哪个布局？',options:[{label:'紧凑'},{label:'宽松'}],answer:'紧凑'}]}]},
        {type:'tool_use',id:'search',name:'Grep',ops:[{kind:'search',pattern:'^(a+)+$',matches:1,files:1,hits:[{path:'fixture.txt',lines:[{line:1,text:'a'.repeat(32)+'b'}]}]}]},
        {type:'tool_use',id:'recorded-exit',name:'bash',ops:[{kind:'shell',intent:'run',head:'python3 --version',command:'python3 --version'}]},
        {type:'tool_use',id:'no-matches',name:'exec_command',ops:[{kind:'shell',intent:'search',search_exit_one:true,steps:[{kind:'search',pattern:'needle'}]}]},
        {type:'tool_use',id:'script',name:'Bash',input:{command:"python3 - <<'PY'\nprint(42)\nPY"},ops:[{kind:'shell',intent:'script',head:'python3 -',command:"python3 - <<'PY'\nprint(42)\nPY",script:{lang:'Python',lines:1,first:'print(42)',body:'print(42)'}}]},
        {type:'tool_use',id:'denied',name:'Bash',ops:[{kind:'shell',intent:'run',head:`python3 ${'long-argument '.repeat(60)}`}]},
        {type:'tool_use',id:'mcp',name:'mcp__fixture__inspect',input:{query:'example',limit:10},ops:[{kind:'mcp',server:'fixture',tool:'inspect'}]},
        {type:'tool_use',id:'find',name:'Glob',input:{pattern:'*.md'},ops:[{kind:'find',pattern:'*.md'}]},
        ...[
          ['write','printf x > output.txt','write'],
          ['append','printf x >> output.txt','write'],
          ['modify','sed -i s/x/y/ output.txt','modify'],
        ].map(([id,command,intent])=>({type:'tool_use',id,name:'Bash',input:{command},ops:[{kind:'shell',intent,head:command,command,target:'output.txt',append:id==='append'}]})),
      ]}},{type:'user',message:{content:[
        {type:'tool_result',tool_use_id:'skill',content:'---\nname: fixture\n---\n# 技能标题\n**技能正文**'},
        {type:'tool_result',tool_use_id:'ask',content:'**紧凑**'},
        {type:'tool_result',tool_use_id:'recorded-exit',is_error:true,exit_code:7,content:'Exit code 2\n退出码：7'},
        {type:'tool_result',tool_use_id:'no-matches',is_error:true,exit_code:1,empty_command_output:true},
        {type:'tool_result',tool_use_id:'script',content:'42'},
        {type:'tool_result',tool_use_id:'denied',is_error:true,deferred:'denied-output',error_summary:"The user doesn't want to proceed with this tool use."},
        {type:'tool_result',tool_use_id:'mcp',content:'Example result'},
        {type:'tool_result',tool_use_id:'find',deferred:'find-output'},
      ]}}],
      safe:[assistant('a-safe','## 安全样例\n\n<script>window.__PWNED__=true</script>\n\n![图片说明](https://markdown-image.invalid/pixel.png)\n\n[危险](javascript:alert(1))\n\n<img src="https://markdown-image.invalid/html.png" onerror="window.__PWNED__=true">')],
    };
    if(long)messages.long=[user('u-first','## 第一条指令'),...Array.from({length:6000},(_,i)=>assistant(`f-${i}`,`## 第 ${i} 条消息\n\n正文 **重点 ${i}**`)),assistant('a-last','# 末尾标题\n\n'+'填充段落 '.repeat(1200)+'\n\n## 搜索段\n\n**最终目标针**\n\n'+'结尾段落 '.repeat(1200))];
    const names={md:'Markdown 示例',plain:'普通文本示例',tools:'工具与思考',cards:'语义工具示例',safe:'安全渲染示例',long:'长会话示例'};
    const records=Object.keys(messages).map(id=>({sessionId:id,projectDir:'p',cwd:'/tmp/md',title:names[id],model:null,firstTs:null,lastTs:null,userCount:1,assistantCount:1,realUserCount:1,assistantTextCount:1,sizeBytes:100,hasTranscript:true}));
    const state={calls:[],messages};window.__MD__=state;
    window.__TAURI_INTERNALS__={transformCallback:cb=>cb,invoke:async(cmd,args={})=>{
      state.calls.push({cmd,args});
      if(cmd==='list_agents')return[{id:'codex',displayName:'Codex',installed:true,supportsFork:false,deletePolicy:'trash'}];
      if(cmd==='catalog')return{sessions:records,projects:[{dirName:'p',displayPath:'/tmp/md',sessionCount:records.length,lastActivity:null}],warnings:[]};
      if(cmd==='read_session')return{meta:records.find(s=>s.sessionId===args.sessionId),events:messages[args.sessionId]};
      if(cmd==='read_tool_payload'){
        if(args.token==='find-output')return await new Promise(resolve => { state.finishFind = () => resolve({type:'tool_result',content:'Chunk ID: test\nWall time: 1s\nOutput:\ndocs/example.md',command_output:'docs/example.md'}); });
        if(args.token==='denied-output')return {type:'tool_result',tool_use_id:'denied',content:"The user doesn't want to proceed with this tool use."};
        if(args.token==='ask-input')return {type:'tool_use',id:'ask',name:'AskUserQuestion',input:{context:'完整提问参数'}};
        return args.token==='tool-input'?{type:'tool_use',id:'call',name:'fixture',input:{value:'**原样**',command:'git status --short'}}:{type:'tool_result',tool_use_id:'call',content:'# 结果标题\n**保持原文**'};
      }
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
  const tool = page.getByRole('button',{name:/^工具调用\s*查看运行日志\s*fixture$/});
  await tool.waitFor();
  assert.equal(await tool.locator('.collapsible-type[title="工具调用"] svg.lucide-wrench').count(),1);
  const thinking = page.getByRole('button',{name:/思考过程.*思考标题/});
  assert.equal(await thinking.locator('.collapsible-type[title="思考过程"] svg.lucide-brain').count(),1);
  const activity=page.locator('.event-group').filter({has:tool});
  assert.deepEqual(await activity.locator(':scope > .collapsible').evaluateAll(cards=>cards.map(card=>card.className)),
    ['collapsible thinking','collapsible tool op generic']);
  assert.deepEqual(await activity.locator(':scope > .thinking').evaluate(el=>{
    const style=getComputedStyle(el);
    return [style.borderLeftWidth,style.borderLeftStyle,style.borderRightWidth,style.borderRightStyle];
  }),['1px','dashed','1px','dashed']);
  const toolVisibility=page.getByRole('checkbox',{name:'工具调用',exact:true});
  await toolVisibility.uncheck();
  await tool.waitFor({state:'detached'});
  await toolVisibility.check();
  await tool.waitFor();
  await page.getByText('样例助手正文', {exact:true}).waitFor();
  assert.equal(await page.evaluate(()=>window.__MD__.calls.filter(c=>c.cmd==='read_tool_payload').length),0);
  await tool.click();
  const fixtureTool=activity.locator('.collapsible.tool');
  await fixtureTool.locator('.tool-input-code').waitFor();
  assert.ok(await tool.getByText('查看运行日志',{exact:true}).isVisible());
  assert.equal(await fixtureTool.locator('.tool-command').count(),0);
  await thinking.click();
  await page.locator('.collapsible.thinking .markdown-body h2').waitFor();
  assert.equal(await page.locator('.collapsible .markdown-body').count(),1);
  assert.ok((await fixtureTool.locator('.tool-payload[data-kind="output"] pre.code').textContent()).includes('# 结果标题\n**保持原文**'));
  assert.ok((await page.locator('.collapsible.tool pre.code').first().textContent()).includes('**原样**'));
  await toggle.uncheck();assert.equal(await page.locator('.collapsible .markdown-body').count(),0);
  assert.ok((await page.locator('.collapsible-body').allTextContents()).join('\n').includes('**保持原文**'));
  await toggle.check();await page.locator('.collapsible.thinking .markdown-body h2').waitFor();
  assert.equal(await page.locator('.collapsible .markdown-body').count(),1);
  assert.equal(await page.evaluate(()=>window.__MD__.calls.filter(c=>c.cmd==='read_tool_payload').length),2);
  const standalone=page.locator('.event-group > .collapsible.command').filter({hasText:'Bash'});
  const failedCommand=standalone.getByRole('button',{name:/^Git\s*git status --short\s*失败 exit 2\s*Bash$/});
  await failedCommand.waitFor();
  await failedCommand.click();
  assert.deepEqual(await standalone.locator(':scope > .collapsible-body > .tool-payload').evaluateAll(sections=>
    sections.map(section=>section.dataset.kind)),['input','error']);
  assert.equal(await standalone.locator('.tool-command .code').textContent(),'git status --short');
  await standalone.getByRole('button',{name:'复制命令',exact:true}).click();
  await page.waitForFunction(()=>window.__MD__.calls.some(call=>call.cmd==='plugin:clipboard-manager|write_text'&&call.args.text==='git status --short'));
  assert.equal(await standalone.locator('.tool-line-numbers').count(),0);
  const expandOutput=standalone.getByRole('button',{name:'展开全部 31 行',exact:true});
  const expandStyle=button=>button.evaluate(el=>{
    const style=getComputedStyle(el), parentStyle=getComputedStyle(el.parentElement);
    const bounds=el.getBoundingClientRect(), parent=el.parentElement.getBoundingClientRect();
    const left=parent.left+parseFloat(parentStyle.borderLeftWidth)+parseFloat(parentStyle.paddingLeft);
    const right=parent.right-parseFloat(parentStyle.borderRightWidth)-parseFloat(parentStyle.paddingRight);
    return {
      centered:Math.abs((bounds.left+bounds.right-left-right)/2)<1,
      height:bounds.height,padding:style.padding,border:style.borderWidth,
      radius:style.borderRadius,fontSize:style.fontSize,
    };
  });
  const outputStyle=await expandStyle(expandOutput);
  assert.ok(outputStyle.centered);
  for(const role of ['user','assistant']){
    const showMore=page.locator(`.msg.${role}`).getByRole('button',{name:'显示更多',exact:true});
    assert.deepEqual(await expandStyle(showMore),outputStyle);
  }
  await expandOutput.click();
  assert.equal(await standalone.locator('.tool-output').getAttribute('data-expanded'),'true');
  await standalone.getByRole('button',{name:'复制错误输出',exact:true}).click();
  await page.waitForFunction(()=>window.__MD__.calls.some(call=>call.cmd==='plugin:clipboard-manager|write_text'&&
    call.args.text===['Exit code 2',...Array.from({length:30},(_,i)=>`output line ${i+1}`)].join('\n')));
  const runCommand=page.locator('.event-group > .collapsible.command').filter({hasText:'run_command'});
  await runCommand.getByRole('button',{name:/^Git 查看改动\s*git diff --stat\s*\+1 步\s*run_command$/}).click();
  await runCommand.locator('.tool-command .code').waitFor();
  assert.equal(await runCommand.locator('.tool-payload[data-kind="input"] .tool-payload-title').textContent(),'输入');
  assert.equal(await runCommand.locator('.tool-payload-note').textContent(),'检查仓库变更');
  assert.equal(await runCommand.locator('.tool-command .code').textContent(),'git diff --stat && ls -la');
  assert.equal(await runCommand.locator('.tool-command .tool-token-operator').textContent(),'&&');
  await runCommand.getByRole('button',{name:'复制命令',exact:true}).click();
  await page.waitForFunction(()=>window.__MD__.calls.some(call=>call.cmd==='plugin:clipboard-manager|write_text'&&call.args.text==='git diff --stat && ls -la'));
  await runCommand.getByRole('tab',{name:'JSON',exact:true}).click();
  assert.ok((await runCommand.locator('.tool-input-code').textContent()).includes('"CommandLine"'));
  console.log(`PASS: Markdown only for thinking, tool output stays plain; command cards with unnumbered output, raw input and full copy (${engine})`);

  await page.locator('.session-card',{hasText:'语义工具示例'}).click();
  const skill=page.locator('.collapsible.skill');
  await skill.locator('.collapsible-head').click();
  const skillRaw=skill.getByRole('tab',{name:'原文',exact:true});
  await skill.getByRole('heading',{name:'技能标题'}).waitFor();
  await skillRaw.click();
  assert.equal(await skillRaw.getAttribute('aria-selected'),'true');
  assert.equal(await skill.locator('.tool-payload[data-kind="output"] pre.code').textContent(),'---\nname: fixture\n---\n# 技能标题\n**技能正文**');
  await skill.getByRole('tab',{name:'JSON',exact:true}).click();
  assert.equal(JSON.parse(await skill.locator('.tool-input-code').textContent()).args,'--brief');
  await skill.getByRole('tab',{name:'说明',exact:true}).click();
  await skill.getByRole('heading',{name:'技能标题'}).waitFor();
  const ask=page.locator('.collapsible.ask');
  await ask.locator('.collapsible-head').click();
  assert.equal(await ask.locator('.ask-question .chosen').textContent(),'紧凑');
  assert.equal(await page.evaluate(()=>window.__MD__.calls.some(call=>call.args.token==='ask-input')),false);
  await ask.getByRole('tab',{name:'JSON',exact:true}).click();
  await ask.locator('.tool-input-code').waitFor();
  assert.equal(JSON.parse(await ask.locator('.tool-input-code').textContent()).context,'完整提问参数');
  assert.equal(await ask.locator('.tool-payload[data-kind="output"] pre.code').textContent(),'**紧凑**');
  await ask.getByRole('tab',{name:'问题',exact:true}).click();
  assert.equal(await ask.locator('.ask-question .chosen').textContent(),'紧凑');
  const grep=page.locator('.collapsible.search').filter({hasText:'Grep'});
  await grep.locator('.collapsible-head').click();
  assert.equal(await grep.locator('.search-text').textContent(),'a'.repeat(32)+'b');
  const noMatches=page.locator('.collapsible.search').filter({hasText:'exec_command'});
  assert.equal(await noMatches.locator('.op-chip').textContent(),'无匹配');
  assert.equal(await noMatches.locator('.tool-status').count(),0);
  await page.getByRole('button',{name:/python3 --version\s*失败 exit 7/}).waitFor();
  assert.deepEqual(await page.locator('.collapsible.command:is(.write, .modify) .op-chip').allTextContents(),['写入','追加','原地修改']);
  const script = page.locator('.collapsible.command.script');
  await script.locator('.collapsible-head').click();
  assert.equal(await script.locator(':scope > .collapsible-body > section').count(), 2);
  assert.equal(await script.getByRole('tabpanel',{name:'Python'}).locator('pre.code').textContent(),'print(42)');
  await script.getByRole('tab',{name:'Python'}).press('ArrowRight');
  assert.equal(await script.getByRole('tabpanel',{name:'命令'}).locator('pre.code').textContent(),"python3 - <<'PY'\nprint(42)\nPY");
  await script.getByRole('tab',{name:'命令'}).press('End');
  assert.equal(JSON.parse(await script.getByRole('tabpanel',{name:'JSON'}).locator('pre.code').textContent()).command,"python3 - <<'PY'\nprint(42)\nPY");
  assert.equal(await script.locator('[data-kind="output"] pre.code').textContent(),'42');
  const mcp = page.locator('.collapsible.mcp');
  await mcp.locator('.collapsible-head').click();
  assert.deepEqual(await mcp.locator('.tool-arguments dt').allTextContents(),['query','limit']);
  await mcp.getByRole('tab',{name:'JSON'}).click();
  assert.equal(JSON.parse(await mcp.locator('.tool-input-code').textContent()).limit, 10);
  const find=page.locator('.collapsible.find');
  assert.equal(await page.evaluate(()=>window.__MD__.calls.some(call=>call.args.token==='find-output')),false);
  await find.locator('.collapsible-head').click();
  assert.equal(await find.locator('[data-kind="output"] [role="status"]').textContent(),'正在读取…');
  await page.evaluate(()=>window.__MD__.finishFind());
  await find.getByRole('tabpanel',{name:'路径'}).getByText('docs/example.md').waitFor();
  await find.getByRole('tab',{name:'原文'}).click();
  assert.match(await find.getByRole('tabpanel',{name:'原文'}).locator('pre.code').textContent(),/^Chunk ID: test/);
  const rejected = page.locator('.collapsible.command').filter({hasText:'已拒绝'});
  const rejectHead = rejected.locator('.collapsible-head');
  assert.match(await rejected.locator('.tool-status').getAttribute('title'), /doesn't want to proceed/);
  for (const width of [960, 620, 520]) {
    await rejectHead.evaluate((head, width) => { head.closest('.collapsible').style.width = `${width}px`; }, width);
    assert.ok(await rejectHead.evaluate(head => {
      const tool = head.querySelector('.tool-tag');
      return tool.getBoundingClientRect().width > 0 && tool.getBoundingClientRect().right <= head.getBoundingClientRect().right && head.scrollHeight <= head.clientHeight;
    }));
  }
  await rejectHead.evaluate(head => { head.closest('.collapsible').style.removeProperty('width'); });
  await rejectHead.click();
  await rejected.locator('[data-kind="error"] pre.code').waitFor();
  assert.equal(await rejected.locator('[data-kind="error"] pre.code').textContent(),"The user doesn't want to proceed with this tool use.");
  console.log(`PASS: Skill and semantic cards switch to raw data; redirect labels distinguish write and append (${engine})`);

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
