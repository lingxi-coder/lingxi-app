/**
 * Injectable time source. Everything in the remote stack that waits (RTDB
 * reconnects, tunnel flushes, RPC timeouts) takes a {@link Clock} so tests
 * can drive time by hand; production code uses {@link systemClock}.
 */

/** Wall clock plus one-shot timers. Handles are opaque to callers. */
export interface Clock {
  /** Milliseconds since the Unix epoch. */
  now(): number;
  /** Run `fn` once after `ms`; returns a handle for {@link Clock.clearTimeout}. */
  setTimeout(fn: () => void, ms: number): unknown;
  /** Cancel a pending timer; unknown or already-fired handles are ignored. */
  clearTimeout(handle: unknown): void;
}

/** The real clock (`Date.now` and the global timers). */
export const systemClock: Clock = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/**
 * A clock that only moves when {@link ManualClock.advance} is called, for tests.
 * Timers fire in due-time order (ties in scheduling order); a timer scheduled
 * by a callback fires within the same `advance` if it falls inside the window.
 */
export class ManualClock implements Clock {
  private time: number;
  private nextId = 1;
  private readonly timers = new Map<number, { at: number; fn: () => void }>();

  constructor(start = 0) {
    this.time = start;
  }

  now(): number {
    return this.time;
  }

  setTimeout(fn: () => void, ms: number): unknown {
    const id = this.nextId++;
    this.timers.set(id, { at: this.time + Math.max(0, ms), fn });
    return id;
  }

  clearTimeout(handle: unknown): void {
    this.timers.delete(handle as number);
  }

  /** Number of timers still pending. */
  get pending(): number {
    return this.timers.size;
  }

  /** Move time forward by `ms`, running every timer that becomes due. */
  advance(ms: number): void {
    const target = this.time + ms;
    for (;;) {
      let nextId = -1;
      let nextAt = Infinity;
      for (const [id, timer] of this.timers) {
        if (timer.at <= target && timer.at < nextAt) {
          nextAt = timer.at;
          nextId = id;
        }
      }
      if (nextId < 0) break;
      const timer = this.timers.get(nextId)!;
      this.timers.delete(nextId);
      this.time = Math.max(this.time, timer.at);
      timer.fn();
    }
    this.time = target;
  }
}
