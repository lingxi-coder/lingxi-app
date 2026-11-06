import { useEffect, useState, useSyncExternalStore } from 'react';
import type { BotsApi } from '../../shared/bots';
import { BotLeaveGuard } from './botLeaveGuard';
import { BotsController, type BotsState } from './botsStore';
import type { BotCreationProposal } from './botCreation';

/** What the Bots page shows. The tab inside a Bot is the page's own business. */
export type BotsView =
  | { kind: 'list' }
  | { kind: 'create'; proposal?: BotCreationProposal; requestId?: number }
  /** `deleteRequest` counts the times the sidebar has asked for the Bot to be deleted: the page asks whether to, each time. */
  | { kind: 'detail'; botId: string; deleteRequest?: number };

export interface UseBots {
  controller: BotsController;
  state: BotsState;
  /** The host said Bots can be used here; until it has answered, they cannot. */
  available: boolean;
  view: BotsView;
  setView(view: BotsView): void;
  /** Asks about the edits of the Bot on screen before anything takes the owner away from them: the page's Back button, the sidebar, the rail, the View menu. */
  guard: BotLeaveGuard;
}

/** The Bots of this computer for the whole window: the sidebar needs to know whether to offer them, the page what they are. */
export function useBots(api: BotsApi | undefined): UseBots {
  const [controller] = useState(() => new BotsController(api));
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot, controller.getSnapshot);
  const [view, setView] = useState<BotsView>({ kind: 'list' });
  // Giving a Bot's edits up is leaving its page for the list; where the owner goes from there is up to the way out that asked.
  const [guard] = useState(() => new BotLeaveGuard(() => setView({ kind: 'list' })));
  useEffect(() => { void controller.start(); }, [controller]);
  return { controller, state, available: state.availability?.available === true, view, setView, guard };
}
