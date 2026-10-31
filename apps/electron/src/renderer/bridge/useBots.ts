import { useEffect, useState, useSyncExternalStore } from 'react';
import type { BotsApi } from '../../shared/bots';
import { BotsController, type BotsState } from './botsStore';

/** What the Bots page shows. The tab inside a Bot is the page's own business. */
export type BotsView =
  | { kind: 'list' }
  | { kind: 'create' }
  | { kind: 'detail'; botId: string };

export interface UseBots {
  controller: BotsController;
  state: BotsState;
  /** The host said Bots can be used here; until it has answered, they cannot. */
  available: boolean;
  view: BotsView;
  setView(view: BotsView): void;
}

/** The Bots of this computer for the whole window: the sidebar needs to know whether to offer them, the page what they are. */
export function useBots(api: BotsApi | undefined): UseBots {
  const [controller] = useState(() => new BotsController(api));
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot, controller.getSnapshot);
  const [view, setView] = useState<BotsView>({ kind: 'list' });
  useEffect(() => { void controller.start(); }, [controller]);
  return { controller, state, available: state.availability?.available === true, view, setView };
}
