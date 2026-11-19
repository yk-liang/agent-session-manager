export interface RefreshClock {
  now: () => number;
  later: (callback: () => void, delay: number) => unknown;
  cancel: (timer: unknown) => void;
}

const clock: RefreshClock = {
  now: () => performance.now(),
  later: (callback, delay) => setTimeout(callback, delay),
  cancel: (timer) => clearTimeout(timer as ReturnType<typeof setTimeout>),
};

export class AutoRefreshScheduler {
  private active = false;
  private blocked = false;
  private busy = false;
  private busyStarted: number | null = null;
  private dirty = false;
  private running = false;
  private disposed = false;
  private quietUntil = 0;
  private notBefore = 0;
  private timer: unknown = null;
  private refresh: () => Promise<void>;
  private clock: RefreshClock;

  constructor(refresh: () => Promise<void>, refreshClock = clock) {
    this.refresh = refresh;
    this.clock = refreshClock;
  }

  setActive(active: boolean) {
    if (this.active === active) return;
    this.active = active;
    this.schedule();
  }

  setBlocked(blocked: boolean) { this.blocked = blocked; this.schedule(); }
  setBusy(busy: boolean) {
    if (this.busy === busy) return;
    this.busy = busy;
    if (!this.running) {
      if (busy) this.busyStarted = this.clock.now();
      else if (this.busyStarted !== null) {
        const elapsed = Math.max(0, this.clock.now() - this.busyStarted);
        this.notBefore = this.clock.now() + Math.max(5000, elapsed * 19);
        this.busyStarted = null;
      }
    }
    this.schedule();
  }

  request(reason: "change" | "focus") {
    if (this.disposed) return;
    this.dirty = true;
    // 等待流式写入停顿，避免反复扫描大日志。
    this.quietUntil = Math.max(this.quietUntil, this.clock.now() + (reason === "change" ? 1200 : 150));
    this.schedule();
  }

  dispose() { this.disposed = true; this.cancelTimer(); }

  private cancelTimer() {
    if (this.timer !== null) this.clock.cancel(this.timer);
    this.timer = null;
  }

  private schedule() {
    this.cancelTimer();
    if (this.disposed || !this.active || this.blocked || this.busy || this.running || !this.dirty) return;
    this.timer = this.clock.later(() => { this.timer = null; void this.run(); },
      Math.max(0, Math.max(this.quietUntil, this.notBefore) - this.clock.now()));
  }

  private async run() {
    if (this.disposed || !this.active || this.blocked || this.busy || this.running || !this.dirty) return;
    this.dirty = false;
    this.running = true;
    const started = this.clock.now();
    try { await this.refresh(); }
    catch { /* 等待下一次文件变更或窗口聚焦后重试。 */ }
    finally {
      const elapsed = Math.max(0, this.clock.now() - started);
      this.running = false;
      // 将连续自动扫描的时间占比限制在约 5%。
      this.notBefore = this.clock.now() + Math.max(5000, elapsed * 19);
      this.schedule();
    }
  }
}
