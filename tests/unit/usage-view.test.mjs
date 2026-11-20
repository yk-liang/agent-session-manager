import test from 'node:test';
import assert from 'node:assert/strict';
import { distributionSlices, costDisplay, eventCostDisplay, tokenDisplay, groupUsageBy, modelTotals, groupUsageCells, sumModelUsage, sumTokens } from "../../src/usage/metrics.ts";
import { buildTrendSeries, dailySeries, entityColors, usageSeries } from "../../src/usage/trend.ts";
import { dateKey, firstDate, summaryQuery } from "../../src/usage/range.ts";
import { formatRate, formatUsd } from '../../src/usage/format.ts';

const row = (date, model, input, output, extra = 0) => ({
  date,
  model,
  inputTokens: input,
  outputTokens: output,
  cacheReadTokens: 2,
  cacheWriteTokens: 1,
  reasoningTokens: 1,
  unclassifiedTokens: extra,
  totalTokens: input + output + extra,
  partial: false,
});

test('range boundaries follow local calendar dates', () => {
  const now = new Date(2026, 9, 22, 0, 30);
  assert.equal(dateKey(now), '2026-10-22');
  assert.equal(firstDate('today', now), '2026-10-22');
  assert.equal(firstDate('week', now), '2026-10-16');
  assert.equal(firstDate('month', now), '2026-09-23');
  assert.equal(firstDate('all', now), '');
});

test('cache and reasoning breakdowns do not get added again to source totals', () => {
  const data = [row('2026-10-21', 'a', 10, 5, 2), row('2026-10-22', 'a', 20, 10, 3)];
  const totals = sumTokens(data);
  assert.equal(totals.totalTokens, 50);
  assert.equal(totals.cacheReadTokens, 4);
  const grouped = modelTotals(data);
  assert.equal(grouped.length, 1);
  assert.equal(grouped[0].totalTokens, 50);
  assert.equal(grouped[0].key, 'a');
});

test('daily trend fills zero days without altering totals or mutating source rows', () => {
  const rows = [row('2026-10-21', 'a', 10, 5)];
  const series = dailySeries(rows, 'week', new Date(2026, 9, 22));
  assert.equal(series.length, 7);
  assert.equal(series[0].date, '2026-10-16');
  assert.equal(series.at(-1).date, '2026-10-22');
  assert.equal(sumTokens(series).totalTokens, 15);
  assert.equal(rows[0].totalTokens, 15);
  const future = row('2026-10-29', 'future', 30);
  assert.equal(dailySeries([future], 'all', new Date(2026, 9, 27, 12))[0].date, '2026-10-29');
});

test('detail availability survives aggregation and missing data is not turned into measured zero', () => {
  const unknown = row('2026-10-21', 'a', 10, 5);
  const known = { ...row('2026-10-22', 'a', 20, 10), cacheReadRecorded: true, reasoningRecorded: true };
  assert.equal(sumTokens([unknown]).reasoningRecorded, false);
  const combined = sumTokens([unknown, known]);
  assert.equal(combined.reasoningRecorded, true);
  assert.equal(combined.cacheReadRecorded, true);
  assert.equal(combined.cacheWriteRecorded, false);
  assert.equal(modelTotals([unknown, known])[0].key, 'a');
});

test('input and output distinguish unknown, measured zero and a measured lower bound', () => {
  const unknown = { ...row('2026-10-21', 'a', 0, 0, 30), inputRecorded: false, outputRecorded: false, inputIncomplete: true, outputIncomplete: true };
  const zero = { ...row('2026-10-21', 'a', 0, 0), inputRecorded: true, outputRecorded: true, inputIncomplete: false, outputIncomplete: false };
  const known = { ...zero, inputTokens: 100, totalTokens: 100 };
  assert.equal(tokenDisplay(sumTokens([unknown]), 'input').text, '—');
  assert.equal(tokenDisplay(sumTokens([zero]), 'output').text, '0');
  assert.equal(tokenDisplay(sumTokens([unknown, known]), 'input').text, '≥ 100');
  assert.equal(tokenDisplay(modelTotals([unknown, known])[0], 'output').text, '≥ 0');
});

const measured = (model, input, cached, costUsd, extra = {}) => ({
  ...row('2026-10-22', model, input, 10),
  inputRecorded: true,
  outputRecorded: true,
  cacheReadRecorded: true,
  cacheReadTokens: cached,
  requestCount: 1,
  requestCountIncomplete: false,
  costUsd,
  costIncomplete: costUsd === null,
  cacheReadIncomplete: false,
  cacheInputTokens: input,
  cacheInputReadTokens: cached,
  cacheInputWriteTokens: 0,
  cacheExcludedCount: 0,
  ...extra,
});

test('model metrics and footer use weighted cache rates across models, sources and buckets', () => {
  const rows = [
    { ...measured('a', 100, 90, 0.25), agent: 'claude' },
    { ...measured('a', 900, 600, 0), agent: 'codex' },
    measured('b', 1000, 300, 0.5),
  ];
  const models = modelTotals(rows);
  assert.equal(models.find(m => m.key === 'a').requestCount, 2);
  assert.equal(groupUsageCells(models.find(m => m.key === 'a'))[3].text, '69.0%');
  const total = sumModelUsage(rows);
  const cells = groupUsageCells(total);
  assert.deepEqual(cells.map(c => c.text), ['2K', '30', '990', '49.5%', '$0.75']);
  assert.equal(total.totalTokens, 2030);
  assert.equal(rows[0].requestCount, 1);
});

test('model metrics distinguish custom and reference estimates, known zero and unknown costs', () => {
  const known = measured('a', 100, 90, 0);
  const missing = { ...measured('a', 900, 0, null), cacheReadRecorded: false, cacheReadIncomplete: true, cacheInputTokens: null, cacheInputReadTokens: 0, cacheExcludedCount: 1 };
  const cells = groupUsageCells(sumModelUsage([known, missing]));
  assert.deepEqual(cells.map(c => c.text), ['1K', '20', '≥ 90', '90.0%', '≥ $0.00']);
  const bucket = usageSeries([known, missing], 'all', new Date(2026, 9, 22, 12));
  const stack = buildTrendSeries([known, missing], bucket, 'all', 'tokens', 'category', (_, key) => key);
  assert.deepEqual(stack.map(({ key, values }) => [key, values[0]]), [
    ['uncached_input', 10], ['cache_read', 90], ['input', 900], ['output', 20],
  ]);
  assert.equal(groupUsageCells(sumModelUsage([missing]))[2].text, '—');
  assert.equal(groupUsageCells(sumModelUsage([missing]))[4].text, '—');
  assert.equal(groupUsageCells(sumModelUsage([measured('zero', 100, 0, 0)]))[3].text, '0.0%');
  assert.equal(groupUsageCells(sumModelUsage([measured('zero', 0, 0, 0)]))[3].text, '—');

  const reference = measured('reference', 1000, 0, 1.25, { referenceCostUsd: 1.25 });
  const custom = measured('custom', 1000, 0, 0.5, { customCostUsd: 0.5 });
  const mixed = costDisplay(sumModelUsage([reference, custom]));
  assert.equal(mixed.text, '$1.75');
  assert.match(mixed.title, /参考价 \$1\.25/);
  assert.match(mixed.title, /自定义价格 \$0\.50/);

  assert.equal(formatRate(14, 'CNY'), '¥14');
  assert.equal(formatRate(2.5, 'USD'), '$2.5');
  assert.equal(formatRate(null, 'USD'), '—');
  assert.equal(formatUsd(0), '$0.00');
  assert.equal(formatUsd(0.00000001), '< $0.000001');
  assert.equal(formatUsd(12_345.67), '$12,345.67');

  const evtFree = eventCostDisplay({ cost: { totalUsd: 0, source: 'builtin' } });
  assert.equal(evtFree.text, '$0.00');
  assert.equal(evtFree.note, '内置');
  const evtCustom = eventCostDisplay({ cost: { totalUsd: 0.5, source: 'custom', currency: 'CNY' } });
  assert.equal(evtCustom.text, '$0.50');
  assert.match(evtCustom.title, /1 USD = 7 CNY/);
  const evtUnknown = eventCostDisplay({ cost: null });
  assert.equal(evtUnknown.text, '—');
});

test('high-cardinality distribution and trend series aggregate tail models into others', () => {
  const now = new Date(2026, 9, 27, 17, 30);
  const rows = Array.from({ length: 9 }, (_, i) => ({
    ...measured(`model-${i + 1}`, (10 - i) * 100, 0, (10 - i) * 0.1),
    agent: i % 2 === 0 ? 'claude' : 'codex',
    date: '2026-10-27',
    bucketStart: null,
    recordCount: 1,
    requestCount: i === 8 ? 20 : 1,
  }));
  const grouped = groupUsageBy(rows, 'models');
  const slices = distributionSlices(grouped, 'tokens', 6);
  assert.equal(slices.length, 7);
  assert.equal(slices.at(-1).othersCount, 3);
  const points = usageSeries(rows, 'week', now);
  const series = buildTrendSeries(rows, points, 'week', 'tokens', 'models', (_, k) => k);
  assert.equal(series.length, 7);
  assert.equal(series.at(-1).label, '其他 3 项');
  assert.deepEqual(series.map(item => item.color), [
    '#4e79a7', '#f28e2c', '#e15759', '#76b7b2', '#59a14f', '#edc949', '#bab0ab',
  ]);

  const byRequests = buildTrendSeries(rows, points, 'week', 'requests', 'models', (_, k) => k);
  const colors = entityColors(grouped.map(item => item.key));
  assert.equal(byRequests[0].key, 'model-9');
  assert.equal(byRequests[0].color, '#9c755f');
  assert.equal(byRequests[0].color, colors.get('model-9'));
});

test('today plots actual hourly buckets and week/month use local calendar boundaries', () => {
  const now = new Date(2026, 9, 27, 17, 30);
  const query = summaryQuery('today', now);
  assert.equal(query.bounds.length, 25);
  const rows = [
    { ...row(dateKey(now), 'a', 100, 10), agent: 'claude', bucketStart: query.bounds[2] },
    { ...row(dateKey(now), 'a', 200, 20), agent: 'claude', bucketStart: query.bounds[15] },
  ];
  const points = usageSeries(rows, 'today', now);
  assert.equal(points.length, 24);
  assert.equal(points[2].totalTokens, 110);
  assert.equal(points[15].totalTokens, 220);
  assert.equal(points[17].totalTokens, 0);
  assert.equal(sumTokens(points).totalTokens, 330);
  assert.equal(summaryQuery('week', now).bounds.length, 8);
  assert.equal(summaryQuery('month', now).bounds.length, 31);
  assert.deepEqual(summaryQuery('all', now), { bounds: [] });
});

test('hourly boundaries preserve spring gaps, autumn repeated hours and quarter-hour timezones', () => {
  const previous = process.env.TZ;
  try {
    process.env.TZ = 'America/Los_Angeles';
    assert.equal(summaryQuery('today', new Date(2026, 2, 8, 12)).bounds.length, 24);
    const autumn = new Date(2026, 10, 1, 12);
    const bounds = summaryQuery('today', autumn).bounds;
    assert.equal(bounds.length, 26);
    const points = usageSeries([], 'today', autumn);
    const repeated = points.filter(p => p.label === '01:00');
    assert.equal(repeated.length, 2);
    assert.notEqual(repeated[0].key, repeated[1].key);
    assert.notEqual(repeated[0].title, repeated[1].title);
    assert.equal(summaryQuery('week', new Date(2026, 2, 10, 12)).bounds.length, 8);
    process.env.TZ = 'Asia/Kathmandu';
    const q = summaryQuery('today', new Date(2026, 9, 27, 12));
    assert.equal(new Date(q.from).getUTCHours(), 18);
    assert.equal(new Date(q.from).getUTCMinutes(), 15);
    assert.equal(q.bounds.length, 25);
  } finally {
    if (previous === undefined) delete process.env.TZ;
    else process.env.TZ = previous;
  }
});
