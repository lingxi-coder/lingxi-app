/**
 * Tells the window that the tasks of some Bots changed. Free of Electron.
 *
 * A task changes several times in a row (taken up, waiting for an answer, a
 * file attached, ended), and the window reads the list again for every word it
 * gets, so the changes of a short while are told together.
 */

export interface AnnouncerTimers {
  start(fn: () => void, milliseconds: number): unknown;
  stop(handle: unknown): void;
}

const systemTimers: AnnouncerTimers = {
  start(fn, milliseconds) {
    const handle = setTimeout(fn, milliseconds);
    // Nothing is lost if the app quits first: there is no window left to tell.
    handle.unref?.();
    return handle;
  },
  stop: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export class TaskChangeAnnouncer {
  private readonly pending = new Set<string>();
  private timer: unknown;

  constructor(
    private readonly announce: (botIds: string[]) => void,
    private readonly delayMs = 150,
    private readonly timers: AnnouncerTimers = systemTimers,
  ) {}

  /** The tasks of these Bots changed. */
  changed(botIds: readonly string[]): void {
    for (const botId of botIds) this.pending.add(botId);
    if (this.timer !== undefined || this.pending.size === 0) return;
    this.timer = this.timers.start(() => {
      this.timer = undefined;
      this.flush();
    }, this.delayMs);
  }

  /** Tell what has changed so far, without waiting for the end of the delay. */
  flush(): void {
    if (this.timer !== undefined) {
      this.timers.stop(this.timer);
      this.timer = undefined;
    }
    if (this.pending.size === 0) return;
    const botIds = [...this.pending].sort();
    this.pending.clear();
    this.announce(botIds);
  }
}
