/**
 * The Bots of this computer, as the window holds them: whether Bots can be used
 * here at all, and the list. Failing to ask is the same answer as "no": a
 * window whose host cannot say has no Bots to show.
 */

import type { BotUi, BotsApi, BotsAvailability } from '../../shared/bots';
import { ipcErrorMessage } from './botForm';
import { ObservableState } from './observableState';

export interface BotsState {
  /** Unknown until the host has answered. */
  availability: BotsAvailability | undefined;
  bots: readonly BotUi[];
  loading: boolean;
  /** Why the list could not be read; empty when the host gave no reason. */
  error: string | undefined;
}

export class BotsController extends ObservableState<BotsState> {
  private request = 0;

  constructor(private readonly api: Pick<BotsApi, 'availability' | 'list'> | undefined) {
    super({ availability: undefined, bots: [], loading: false, error: undefined });
  }

  /** Ask whether Bots can be used here and, if so, read them. */
  async start(): Promise<void> {
    if (!this.api) {
      this.update({ availability: { available: false } });
      return;
    }
    let availability: BotsAvailability;
    try { availability = await this.api.availability(); }
    catch (error) {
      this.update({ availability: { available: false, reason: ipcErrorMessage(error) } });
      return;
    }
    this.update({ availability });
    if (availability.available) await this.reload();
  }

  async reload(): Promise<void> {
    if (!this.api || !this.getSnapshot().availability?.available) return;
    const request = ++this.request;
    this.update({ loading: true });
    try {
      const bots = await this.api.list();
      if (request === this.request) this.update({ bots, error: undefined, loading: false });
    } catch (error) {
      if (request === this.request) this.update({ error: ipcErrorMessage(error), loading: false });
    }
  }

  /** Take in a Bot the host just returned, whether new or changed. */
  put(bot: BotUi): void {
    const { bots } = this.getSnapshot();
    this.update({ bots: bots.some((known) => known.botId === bot.botId) ? bots.map((known) => known.botId === bot.botId ? bot : known) : [...bots, bot] });
  }

  remove(botId: string): void {
    this.update({ bots: this.getSnapshot().bots.filter((bot) => bot.botId !== botId) });
  }
}
