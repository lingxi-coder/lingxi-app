import { useEffect, useState } from 'react';
import type { BotsApi } from '../../shared/bots';
import { BotSessionsController } from './botSessionsStore';

/** What a window with no host to ask has: no Bot, so nothing to list and nothing to hear. */
const NO_BOTS: Pick<BotsApi, 'listSessions' | 'onPromptSent' | 'onSessionShown'> = {
  listSessions: () => Promise.reject(new Error('Bots are not available.')),
  onPromptSent: () => () => undefined,
  onSessionShown: () => () => undefined,
};

/** The chats of the Bots on show in the sidebar, for the whole window: they stay read while the sidebar lists something else. */
export function useBotSessions(api: BotsApi | undefined): BotSessionsController {
  const [controller] = useState(() => new BotSessionsController(api ?? NO_BOTS));
  useEffect(() => controller.watch(), [controller]);
  return controller;
}
