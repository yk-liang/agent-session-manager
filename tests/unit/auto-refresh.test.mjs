import test from "node:test";
import assert from "node:assert/strict";
import { AutoRefreshScheduler } from "../../src/autoRefresh.ts";

function fakeClock() {
  let now = 0, nextId = 0;
  const timers = new Map();
  const clock = { now: () => now, later: (callback, delay) => {
    const id = ++nextId; timers.set(id, { at: now + delay, callback }); return id;
  }, cancel: id => timers.delete(id) };
  return { clock, timers, spend: ms => { now += ms; }, tick: async ms => {
    const end = now + ms;
    for (;;) {
      const next = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
      if (!next || next[1].at > end) break;
      now = Math.max(now, next[1].at); timers.delete(next[0]); next[1].callback();
      await Promise.resolve(); await Promise.resolve();
    }
    now = Math.max(now, end);
    await Promise.resolve(); await Promise.resolve();
  } };
}

test("idle foreground has no periodic scans or timers; a write burst produces one refresh", async () => {
  const time = fakeClock(); let scans = 0;
  const scheduler = new AutoRefreshScheduler(async () => { scans++; }, time.clock);
  scheduler.setActive(true); await time.tick(150);
  assert.equal(scans, 1);
  await time.tick(100000); assert.equal(scans, 1); assert.equal(time.timers.size, 0);
  for (let i = 0; i < 1000; i++) { scheduler.request("change"); await time.tick(10); }
  assert.equal(scans, 1);
  await time.tick(1190); assert.equal(scans, 2);
  await time.tick(100000); assert.equal(scans, 2); assert.equal(time.timers.size, 0);
});

test("background, user selection and a manual scan defer updates without losing the dirty state", async () => {
  const time = fakeClock(); let scans = 0;
  const scheduler = new AutoRefreshScheduler(async () => { scans++; }, time.clock);
  scheduler.request("change"); await time.tick(10000); assert.equal(scans, 0);
  scheduler.setActive(true); scheduler.setBusy(true); await time.tick(10000); assert.equal(scans, 0);
  scheduler.setBusy(false); scheduler.setBlocked(true); await time.tick(10000); assert.equal(scans, 0);
  scheduler.setBlocked(false); await time.tick(179999); assert.equal(scans, 0);
  await time.tick(1); assert.equal(scans, 1);
  scheduler.setActive(false); scheduler.request("change"); await time.tick(10000); assert.equal(scans, 1);
  scheduler.setActive(true); await time.tick(150); assert.equal(scans, 2);
});

test("initial and manual reads share the automatic scan budget without dropping file events", async () => {
  const time = fakeClock(); let scans = 0;
  const scheduler = new AutoRefreshScheduler(async () => { scans++; }, time.clock);
  scheduler.setBusy(true);
  scheduler.setActive(true);
  scheduler.request("change");
  await time.tick(1000);
  scheduler.setBusy(true);
  scheduler.setBusy(false);
  await time.tick(18999);
  assert.equal(scans, 0, "the initial read must impose the same quiet budget as an automatic scan");
  await time.tick(1);
  assert.equal(scans, 1, "changes during the initial read must still trigger a later scan");
  await time.tick(100000);
  assert.equal(scans, 1);
  assert.equal(time.timers.size, 0);
});

test("events arriving during a scan queue one later scan and never overlap", async () => {
  const time = fakeClock(); let scans = 0, release;
  const scheduler = new AutoRefreshScheduler(async () => { scans++; if (scans === 1) await new Promise(resolve => { release = resolve; }); }, time.clock);
  scheduler.setActive(true); await time.tick(150); assert.equal(scans, 1);
  for (let i = 0; i < 100; i++) scheduler.request("change");
  await time.tick(1000); assert.equal(scans, 1);
  release(); await Promise.resolve(); await Promise.resolve();
  await time.tick(18999); assert.equal(scans, 1);
  await time.tick(1); assert.equal(scans, 2);
});

test("slow logs extend the quiet interval to keep automatic scan time within its budget", async () => {
  const time = fakeClock(); let scans = 0;
  const scheduler = new AutoRefreshScheduler(async () => { scans++; time.spend(2000); }, time.clock);
  scheduler.setActive(true); await time.tick(150); assert.equal(scans, 1);
  scheduler.request("change"); await time.tick(37999); assert.equal(scans, 1);
  await time.tick(1); assert.equal(scans, 2);
});

test("disposing a context cancels pending work and old events cannot revive it", async () => {
  const time = fakeClock(); let scans = 0;
  const scheduler = new AutoRefreshScheduler(async () => { scans++; }, time.clock);
  scheduler.setActive(true); scheduler.request("change"); scheduler.dispose();
  scheduler.setActive(true); scheduler.request("focus"); await time.tick(100000);
  assert.equal(scans, 0); assert.equal(time.timers.size, 0);
});
