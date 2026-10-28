// 独立进程验证：仅使用临时日志/账本，不修改机器上的 CLI 数据或登录服务。
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtempSync, mkdirSync, writeFileSync, appendFileSync, readFileSync, rmSync, renameSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
const exec = promisify(execFile);
const exe = path.resolve(process.env.ASM_COLLECTOR_BIN || 'src-tauri/target/release/agent-session-manager');
const architecture = process.env.ASM_COLLECTOR_ARCH;
assert.ok(!architecture || ['arm64','x86_64'].includes(architecture), 'Unsupported collector test architecture');
const collectorArgs = ['--usage-collector','--collector-directory'];
const base = mkdtempSync(path.join(tmpdir(), 'asm-collector-'));
const control = path.join(base, 'control'); mkdirSync(control);
const roots = Object.fromEntries(['qoder','claude','codex','opencode','pi'].map(id => [id,[path.join(base,id,'logs')]]));
const settings = {enabled:true,roots};
const saveSettings = () => { writeFileSync(path.join(control,'next.json'),JSON.stringify(settings),{mode:0o600});renameSync(path.join(control,'next.json'),path.join(control,'settings.json')); };
saveSettings();
const family = (id, total) => JSON.stringify({type:'assistant',sessionId:'session',timestamp:new Date().toISOString(),message:{id,role:'assistant',model:'fixture',usage:{input_tokens:total,output_tokens:0},content:'PRIVATE_FIXTURE_BODY'}})+'\n';
let child;
const spawnCollector = stdio => architecture
  ? spawn('/usr/bin/arch',[`-${architecture}`,exe,...collectorArgs,control],{stdio})
  : spawn(exe,[...collectorArgs,control],{stdio});
const start = () => child = spawnCollector(['ignore','ignore','pipe']);
const exited = p => new Promise((resolve,reject) => { const timer=setTimeout(()=>reject(Error('Collector did not exit')),5000); if(p.exitCode!==null){clearTimeout(timer);resolve(p.exitCode);return;}p.once('exit',code=>{clearTimeout(timer);resolve(code);}); });
const until = async (check, timeout=5000) => { const end=performance.now()+timeout;while(performance.now()<end){try{if(await check())return;}catch{}await new Promise(r=>setTimeout(r,40));}throw Error('Collector assertion timed out'); };
const observed = async () => JSON.parse((await exec('python3',['-c',`import sqlite3,json,sys
c=sqlite3.connect(sys.argv[1]);print(json.dumps({a:[s for r in c.execute('SELECT data FROM usage_ledger_sources WHERE agent=?',(a,)) for s in json.loads(r[0])['samples']] for a in ['qoder','claude','codex','opencode','pi']}))`,path.join(control,'ledger.sqlite3')])).stdout);
const cpuMillis = async pid => {const out=(await exec('ps',['-p',String(pid),'-o','time='])).stdout.trim();const t=out.split(':').map(Number);return t.length===2?(t[0]*60+t[1])*1000:(t[0]*3600+t[1]*60+t[2])*1000;};
try {
 for(const id of ['claude','codex','opencode','pi'])mkdirSync(roots[id][0],{recursive:true});
 writeFileSync(path.join(roots.claude[0],'session.jsonl'),family('msg_first',10));
 writeFileSync(path.join(roots.codex[0],'session.jsonl'),[JSON.stringify({type:'session_meta',payload:{id:'codex-fixture'}}),JSON.stringify({type:'event_msg',timestamp:new Date().toISOString(),payload:{type:'token_count',info:{total_token_usage:{input_tokens:20,output_tokens:2,total_tokens:22}}}}),''].join('\n'));
 writeFileSync(path.join(roots.pi[0],'session.jsonl'),[JSON.stringify({type:'session',id:'pi-fixture'}),JSON.stringify({type:'message',id:'reply',timestamp:new Date().toISOString(),message:{role:'assistant',model:'fixture',usage:{input:30,output:3}}}),''].join('\n'));
 await exec('python3',['-c',`import sqlite3,json,sys,time
c=sqlite3.connect(sys.argv[1]);c.executescript('CREATE TABLE session(id TEXT PRIMARY KEY,time_created INTEGER);CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT);');t=int(time.time()*1000);c.execute('INSERT INTO session VALUES (?,?)',('fixture',t-1));c.execute('INSERT INTO message VALUES (?,?,?,?)',('a','fixture',t,json.dumps({'role':'assistant','modelID':'fixture','time':{'created':t},'tokens':{'input':40,'output':4}})));c.commit()`,path.join(roots.opencode[0],'opencode.db')]);
 start();const initialPid=child.pid;
 await until(()=>Object.keys(JSON.parse(readFileSync(path.join(control,'status.json'),'utf8')).collectedAt).length===5);
 const initial=await observed();for(const id of ['claude','codex','opencode','pi'])assert.equal(initial[id].length,1,id);
 const began=performance.now();appendFileSync(path.join(roots.claude[0],'session.jsonl'),family('msg_continued',11));
 await until(async()=> (await observed()).claude.some(s=>s.key==='message:msg_continued'));
 console.log(JSON.stringify({check:'append_without_gui',latency_ms:Math.round(performance.now()-began)}));
 // A data root absent at startup must be watched as soon as it is created.
 mkdirSync(roots.qoder[0],{recursive:true});writeFileSync(path.join(roots.qoder[0],'session.jsonl'),family('msg_qoder',50));
 await until(async()=>(await observed()).qoder.length===1);
 console.log('PASS: all five adapters collect without a window, including a newly created Agent data root');
 const duplicate=spawnCollector('ignore');
 assert.equal(await exited(duplicate),0);assert.equal(child.pid,initialPid);
 console.log('PASS: a second process cannot become another collector');
 const cpuStart=await cpuMillis(child.pid),idleStart=performance.now();
 await new Promise(r=>setTimeout(r,10000));
 const cpuMs=(await cpuMillis(child.pid))-cpuStart;
 const rssKiB=Number((await exec('ps',['-p',String(child.pid),'-o','rss='])).stdout.trim());
 console.log(JSON.stringify({check:'actual_collector_idle',elapsed_ms:Math.round(performance.now()-idleStart),cpu_ms:cpuMs,rss_mib:Math.round(rssKiB/1024*10)/10}));
 const burstStart=performance.now();for(let i=0;i<100;i++)appendFileSync(path.join(roots.claude[0],'session.jsonl'),family(`msg_burst_${i}`,1));
 await until(async()=>(await observed()).claude.length===102);
 console.log(JSON.stringify({check:'100_append_burst',latency_ms:Math.round(performance.now()-burstStart)}));
 for(const value of ['ledger.sqlite3','settings.json','status.json'])assert.equal(statSync(path.join(control,value)).mode&0o777,0o600);
 assert.ok(!readFileSync(path.join(control,'ledger.sqlite3')).includes(Buffer.from('PRIVATE_FIXTURE_BODY')));
 rmSync(path.join(roots.claude[0],'session.jsonl'));child.kill('SIGTERM');assert.equal(await exited(child),0);
 start();await until(()=>JSON.parse(readFileSync(path.join(control,'status.json'),'utf8')).pid===child.pid);
 assert.equal((await observed()).claude.length,102);
 settings.enabled=false;saveSettings();assert.equal(await exited(child),0);
 console.log('PASS: graceful restart and deleted transcripts retain lifetime observations; pause exits and private storage contains no dialogue');
} finally {if(child&&child.exitCode===null){child.kill('SIGTERM');await exited(child).catch(()=>child.kill('SIGKILL'));}rmSync(base,{recursive:true,force:true});}
