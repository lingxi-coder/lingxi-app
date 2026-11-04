import { ObservableState } from './observableState';

export interface BotLeaveState {
  /** A way out is waiting for the owner's answer: the Bot's page shows the question. */
  asking: boolean;
}

/**
 * The question the Bots page asks before it lets go of a Bot's unsaved edits, whoever is taking the owner away:
 * the page's own Back button, or a chat, another Bot, the rail's modes and the View menu. The Bot's page says
 * whether it holds edits (`hold`); a way out (`leave`) then waits for the owner's answer instead of going, and
 * one with nothing to lose just goes.
 */
export class BotLeaveGuard extends ObservableState<BotLeaveState> {
  private dirty = false;
  private way: (() => void) | undefined;

  /** `discard` gives the edits up: the Bot's page goes back to the list of Bots. */
  constructor(private readonly discard: () => void) {
    super({ asking: false });
  }

  /** The Bot's page says whether it has edits that are not saved. With none to lose, or no page, a question still open is dropped. */
  hold(dirty: boolean): void {
    this.dirty = dirty;
    if (!dirty) this.close();
  }

  /** Take `go` at once if nothing would be lost; otherwise ask, and take it only if the owner gives the edits up. A newer way out replaces one that is still waiting. */
  leave(go: () => void): void {
    if (!this.dirty) {
      go();
      return;
    }
    this.way = go;
    if (!this.getSnapshot().asking) this.update({ asking: true });
  }

  /** The owner gives the edits up: the page lets go of them first, then the way out is taken. */
  confirm(): void {
    const go = this.way;
    this.close();
    if (go === undefined) return;
    this.discard();
    go();
  }

  /** The owner stays where they are, with the edits as they were. */
  cancel(): void {
    this.close();
  }

  private close(): void {
    this.way = undefined;
    if (this.getSnapshot().asking) this.update({ asking: false });
  }
}
