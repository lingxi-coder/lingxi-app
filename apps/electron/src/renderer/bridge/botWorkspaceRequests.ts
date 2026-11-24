/**
 * The folders the Bots asked for and the owner has not answered. The host tells
 * the window when a request comes or goes, so the list is read again then and
 * not on a timer. Answering is one call: approving binds the folder to the Bot
 * (read or read and write, as asked), denying only forgets the request.
 */

import type { BotUi, BotWorkspaceRequestUi, BotsApi } from '../../shared/bots';
import { ipcErrorMessage } from './botForm';
import { ObservableState } from './observableState';

export type BotWorkspaceDecision = 'approve' | 'deny';

export interface BotWorkspaceRequestsState {
  /** Oldest first, of every Bot. Unknown until the first read. */
  requests: readonly BotWorkspaceRequestUi[] | undefined;
  loading: boolean;
  error: string | undefined;
  /** The request being answered now: no other is answered meanwhile. */
  deciding: { requestId: string; decision: BotWorkspaceDecision } | undefined;
  /** Why the last answer was not taken, for the request it was about. */
  decideError: { requestId: string; message: string } | undefined;
}

/** The requests of one Bot. */
export function requestsOfBot(state: Pick<BotWorkspaceRequestsState, 'requests'>, botId: string): BotWorkspaceRequestUi[] {
  return (state.requests ?? []).filter((request) => request.botId === botId);
}

export class BotWorkspaceRequestsController extends ObservableState<BotWorkspaceRequestsState> {
  private request = 0;

  constructor(
    private readonly api: BotsApi['workspaceRequests'],
    /** Told of the Bot as it is once a folder was bound to it, so that the list of Bots follows. */
    private readonly onBot: (bot: BotUi) => void,
  ) {
    super({ requests: undefined, loading: false, error: undefined, deciding: undefined, decideError: undefined });
  }

  /** Read the requests. An answer that a later read overtook is dropped. */
  async load(): Promise<void> {
    const request = ++this.request;
    this.update({ loading: true });
    try {
      const requests = await this.api.list();
      if (request === this.request) this.update({ requests, error: undefined, loading: false });
    } catch (error) {
      if (request === this.request) this.update({ error: ipcErrorMessage(error), loading: false });
    }
  }

  /** Read the requests again whenever the host says one came, was answered or ran out. Returns the way to stop. */
  watch(): () => void {
    return this.api.onChanged(() => { void this.load(); });
  }

  /** Answer a request. Resolves to the Bot after an approval, which is what the folder was bound to; `undefined` for a refusal or a failure. */
  async decide(botId: string, requestId: string, decision: BotWorkspaceDecision): Promise<BotUi | undefined> {
    if (this.getSnapshot().deciding !== undefined) return undefined;
    this.update({ deciding: { requestId, decision }, decideError: undefined });
    let bot: BotUi;
    try {
      bot = await this.api.decide(botId, requestId, decision);
    } catch (error) {
      this.update({ deciding: undefined, decideError: { requestId, message: ipcErrorMessage(error) } });
      // The request may be gone already (answered elsewhere, or it ran out): what is left is what the host has.
      await this.load();
      return undefined;
    }
    this.update({ deciding: undefined });
    if (decision === 'approve') this.onBot(bot);
    await this.load();
    return decision === 'approve' ? bot : undefined;
  }
}
