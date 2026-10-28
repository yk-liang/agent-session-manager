import test from "node:test";
import assert from "node:assert/strict";
import { groupSessions, sessionGroup, PROJECTLESS_GROUP } from "../../src/sessionGroups.ts";
import { sessionKey } from "../../src/utils.ts";

test("projectless groups use explicit IDs, keep real projects and preserve source identity", () => {
  const sessions = [
    {sessionId:"a",projectDir:"dir-a",cwd:"/tmp/a",lastTs:"2026-10-16T00:00:00Z"},
    {sessionId:"b",projectDir:"dir-b",cwd:"/tmp/b",lastTs:"2026-10-15T00:00:00Z"},
    {sessionId:"named",projectDir:"dir-a",cwd:"/tmp/a",lastTs:"2026-10-16T01:00:00Z"},
    {sessionId:"manual",projectDir:"manual",cwd:"/Users/demo/Documents/Codex/manual",lastTs:null},
  ];
  const before = JSON.stringify(sessions);
  const keys = sessions.map(sessionKey);
  const ids = new Set(["a","b"]);
  const projects = [
    {dirName:"dir-a",displayPath:"/tmp/a",sessionCount:2,lastActivity:null},
    {dirName:"dir-b",displayPath:"/tmp/b",sessionCount:1,lastActivity:null},
    {dirName:"manual",displayPath:sessions[3].cwd,sessionCount:1,lastActivity:null},
  ];
  const groups = groupSessions(projects,sessions,ids);
  assert.equal(groups.length,3);
  assert.equal(groups[0].dirName,PROJECTLESS_GROUP);
  assert.equal(groups[0].displayPath,"未归属项目");
  assert.equal(groups[0].sessionCount,2);
  assert.equal(groups[0].lastActivity,sessions[0].lastTs);
  assert.equal(groups.find(g=>g.dirName==="dir-a").sessionCount,1);
  assert.equal(groups.reduce((n,g)=>n+g.sessionCount,0),4);
  assert.equal(sessionGroup(sessions[2],ids),"dir-a");
  assert.equal(sessionGroup(sessions[3],ids),"manual");
  assert.equal(JSON.stringify(sessions),before);
  assert.deepEqual(sessions.map(sessionKey),keys);
});

test("missing projectless metadata preserves legacy and other agents' directory groups", () => {
  const projects=[{dirName:"legacy",displayPath:"/tmp/legacy",sessionCount:0,lastActivity:null}];
  const groups=groupSessions(projects,[],new Set());
  assert.deepEqual(groups,[{...projects[0],projectless:false}]);
  assert.deepEqual(projects,[{dirName:"legacy",displayPath:"/tmp/legacy",sessionCount:0,lastActivity:null}]);
});

test("Qoder roots merge run directories while hints distinguish the same ID in different source projects", () => {
  const sessions=[
    {sessionId:"same",projectDir:"run-a",cwd:"/work/real-project/.local/run-a",lastTs:null},
    {sessionId:"b",projectDir:"run-b",cwd:"/work/real-project/.local/run-b",lastTs:null},
    {sessionId:"same",projectDir:"automatic",cwd:"/auto/task",lastTs:null},
    {sessionId:"manual",projectDir:"manual",cwd:"/manual/task",lastTs:null},
  ];
  const before=JSON.stringify(sessions);
  const projects=sessions.map(s=>({dirName:s.projectDir,displayPath:s.cwd,sessionCount:1,lastActivity:null}));
  const hints=new Map([[sessionKey(sessions[0]),"/work/real-project"],[sessionKey(sessions[1]),"/work/real-project"],[sessionKey(sessions[2]),null]]);
  const groups=groupSessions(projects,sessions,new Set(),hints);
  assert.equal(groups.length,3);
  assert.equal(groups[0].displayPath,"未归属项目");
  assert.equal(groups[0].sessionCount,1);
  const root=groups.find(g=>g.displayPath==="/work/real-project");
  assert.equal(root.sessionCount,2);
  assert.equal(sessions.filter(s=>sessionGroup(s,new Set(),hints)===root.dirName).length,2);
  assert.notEqual(sessionGroup(sessions[0],new Set(),hints),sessionGroup(sessions[2],new Set(),hints));
  assert.equal(JSON.stringify(sessions),before);
});
