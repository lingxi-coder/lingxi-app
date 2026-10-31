/**
 * Keeping the computer awake while Bots are at work, if the owner wants that.
 * Free of Electron.
 *
 * Whatever is at work holds the computer awake and lets go when it is done. The
 * holds are counted, and the operating system is asked once for all of them. A
 * hold taken while the owner's preference is off counts all the same, so that
 * turning the preference on keeps the computer awake for work already going on.
 * Closing the lid still puts the computer to sleep: nothing can prevent that.
 */

/** What the app asks of the operating system. */
export interface KeepAwakeOptions {
  /** Begin preventing idle sleep. The id it returns is what `stop` is given. */
  start(): number;
  stop(id: number): void;
  /** Whether the owner wants the computer kept awake. Off if absent. */
  enabled?: boolean;
}

export class KeepAwake {
  private holds = 0;
  private enabled: boolean;
  private blocker: number | undefined;

  constructor(private readonly options: KeepAwakeOptions) {
    this.enabled = options.enabled === true;
  }

  /** Something is at work. The function returned says it is done; calling it again changes nothing. */
  hold(): () => void {
    this.holds += 1;
    this.apply();
    let released = false;
    return () => {
      if (released) return;
      released = true;
      this.holds -= 1;
      this.apply();
    };
  }

  /** The owner turned the preference on or off. Off ends the blocker at once; on begins it if something is at work. */
  setEnabled(enabled: boolean): void {
    this.enabled = enabled;
    this.apply();
  }

  /** The operating system is asked to keep the computer awake exactly while the preference is on and something holds. */
  private apply(): void {
    const wanted = this.enabled && this.holds > 0;
    if (wanted && this.blocker === undefined) {
      this.blocker = this.options.start();
    } else if (!wanted && this.blocker !== undefined) {
      const id = this.blocker;
      this.blocker = undefined;
      this.options.stop(id);
    }
  }
}
