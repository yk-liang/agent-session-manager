import test from 'node:test';
import assert from 'node:assert/strict';
import { dateKey, firstDate, selectRows, sumTokens, dailySeries, modelTotals, formatUsageField, summaryQuery, usageSeries } from '../../src/usageView.ts';
const row=(date,model,input,output,extra=0)=>({date,model,modelName:model,inputTokens:input,outputTokens:output,cacheReadTokens:2,cacheWriteTokens:1,reasoningTokens:1,unclassifiedTokens:extra,totalTokens:input+output+extra,partial:false});
test('range boundaries follow local calendar dates and include unknown dates only in all',()=>{
 const now=new Date(2026,9,22,0,30),report={rows:[row('2026-10-15','a',10,1),row('2026-10-16','a',20,2),row('2026-10-22','b',30,3),row('','a',40,4)]};
 assert.equal(dateKey(now),'2026-10-22');assert.equal(firstDate('week',now),'2026-10-16');
 assert.equal(sumTokens(selectRows(report,'week','all',now)).totalTokens,55);
 assert.equal(sumTokens(selectRows(report,'today','b',now)).totalTokens,33);
 assert.equal(sumTokens(selectRows(report,'all','a',now)).totalTokens,77);
});
test('cache and reasoning breakdowns do not get added again to source totals',()=>{
 const data=[row('2026-10-21','a',10,5,2),row('2026-10-22','a',20,10,3)];
 const totals=sumTokens(data);assert.equal(totals.totalTokens,50);assert.equal(totals.cacheReadTokens,4);
 const grouped=modelTotals(data);assert.equal(grouped.length,1);assert.equal(grouped[0].totalTokens,50);
 assert.equal(grouped[0].model,'a');assert.equal(grouped[0].modelName,'a');assert.equal(grouped[0].partial,false);
});
test('daily trend fills zero days without altering totals or mutating source rows',()=>{
 const rows=[row('2026-10-21','a',10,5)];const series=dailySeries(rows,'week',new Date(2026,9,22));
 assert.equal(series.length,7);assert.equal(series[0].date,'2026-10-16');assert.equal(series.at(-1).date,'2026-10-22');
 assert.equal(sumTokens(series).totalTokens,15);assert.equal(rows[0].totalTokens,15);
});
test('detail availability survives aggregation and missing data is not turned into measured zero',()=>{
 const unknown=row('2026-10-21','a',10,5),known={...row('2026-10-22','a',20,10),cacheReadRecorded:true,reasoningRecorded:true};
 assert.equal(sumTokens([unknown]).reasoningRecorded,false);
 const combined=sumTokens([unknown,known]);assert.equal(combined.reasoningRecorded,true);assert.equal(combined.cacheReadRecorded,true);assert.equal(combined.cacheWriteRecorded,false);
 assert.equal(modelTotals([unknown,known])[0].model,'a');
});
test('input and output distinguish unknown, measured zero and a measured lower bound',()=>{
 const unknown={...row('2026-10-21','a',0,0,30),inputRecorded:false,outputRecorded:false,inputIncomplete:true,outputIncomplete:true};
 const zero={...row('2026-10-21','a',0,0),inputRecorded:true,outputRecorded:true,inputIncomplete:false,outputIncomplete:false};
 const known={...zero,inputTokens:100,totalTokens:100};
 assert.equal(formatUsageField(sumTokens([unknown]),'input'),'未提供');
 assert.equal(formatUsageField(sumTokens([zero]),'output'),'0');
 assert.equal(formatUsageField(sumTokens([unknown,known]),'input'),'≥ 100');
 assert.equal(formatUsageField(modelTotals([unknown,known])[0],'output'),'≥ 0');
});

test('today plots actual hourly buckets and week/month use local calendar boundaries',()=>{
 const now=new Date(2026,9,27,17,30),query=summaryQuery('today',now);
 assert.equal(query.bounds.length,25);
 const rows=[{...row(dateKey(now),'a',100,10),agent:'claude',bucketStart:query.bounds[2]},{...row(dateKey(now),'a',200,20),agent:'claude',bucketStart:query.bounds[15]}];
 const points=usageSeries(rows,'today',now);
 assert.equal(points.length,24);assert.equal(points[2].totalTokens,110);assert.equal(points[15].totalTokens,220);assert.equal(points[17].totalTokens,0);
 assert.equal(sumTokens(points).totalTokens,330);
 assert.equal(summaryQuery('week',now).bounds.length,8);assert.equal(summaryQuery('month',now).bounds.length,31);
 assert.deepEqual(summaryQuery('all',now),{bounds:[]});
});

test('hourly boundaries preserve spring gaps, autumn repeated hours and quarter-hour timezones',()=>{
 const previous=process.env.TZ;
 try{
  process.env.TZ='America/Los_Angeles';
  assert.equal(summaryQuery('today',new Date(2026,2,8,12)).bounds.length,24);
  const autumn=new Date(2026,10,1,12),bounds=summaryQuery('today',autumn).bounds;
  assert.equal(bounds.length,26);
  const points=usageSeries([], 'today',autumn);
  const repeated=points.filter(p=>p.label==='01:00');assert.equal(repeated.length,2);assert.notEqual(repeated[0].key,repeated[1].key);assert.notEqual(repeated[0].title,repeated[1].title);
  assert.equal(summaryQuery('week',new Date(2026,2,10,12)).bounds.length,8);
  process.env.TZ='Asia/Kathmandu';const q=summaryQuery('today',new Date(2026,9,27,12));
  assert.equal(new Date(q.from).getUTCHours(),18);assert.equal(new Date(q.from).getUTCMinutes(),15);assert.equal(q.bounds.length,25);
 }finally{if(previous===undefined)delete process.env.TZ;else process.env.TZ=previous;}
});
