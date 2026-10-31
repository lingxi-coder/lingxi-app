/**
 * Telling a Bot's chats from a project's, on the window's side. The main
 * process marks the runtime of a Bot's session with the Bot's id; until that
 * answer is in, the window waits on a placeholder folder that names the Bot.
 */

import { tr } from '../i18n/locale';
import { ipcErrorMessage } from './botForm';
import type { SessionRuntimeSummary } from './lingxi';
import { formatRelativeSessionTime } from './sessionPresentation';

const PENDING_BOT_PREFIX = 'bot:';

/** The folder a chat of a Bot is given while the host is still starting it. No project path looks like this: those are absolute. */
export function pendingBotPath(botId: string): string {
  return `${PENDING_BOT_PREFIX}${botId}`;
}

export function pendingBotId(projectPath: string | undefined): string | undefined {
  return projectPath?.startsWith(PENDING_BOT_PREFIX) ? projectPath.slice(PENDING_BOT_PREFIX.length) : undefined;
}

type RuntimeBotFields = Pick<SessionRuntimeSummary, 'sessionId' | 'botId'>;

/** The Bot a displayed chat belongs to, if it is a Bot's. */
export function botIdOfSession(
  runtimes: readonly RuntimeBotFields[],
  session: { sessionId: string; projectPath: string } | undefined,
): string | undefined {
  if (!session) return undefined;
  return runtimes.find((runtime) => runtime.sessionId === session.sessionId)?.botId ?? pendingBotId(session.projectPath);
}

/** Whether a session is a Bot's chat: the host marks the runtime of every one. */
export function isBotSession(runtimes: readonly RuntimeBotFields[], sessionId: string): boolean {
  return runtimes.some((runtime) => runtime.sessionId === sessionId && runtime.botId !== undefined);
}

export type BotActivity = 'working' | 'waiting' | 'idle';

/** What one running chat of a Bot is doing. */
export interface BotRuntimeActivity {
  botId: string;
  turnActive: boolean;
  pendingInteractions: number;
  pendingAskUserQuestions: number;
}

/**
 * The running chats of Bots as they are now: the host's summary, overtaken by
 * what the window has heard of each chat since (`live`), which is fresher.
 */
export function botRuntimes(
  runtimes: ReadonlyArray<Pick<SessionRuntimeSummary, 'sessionId' | 'botId' | 'turnActive' | 'pendingInteractions' | 'pendingAskUserQuestions'>>,
  live: (sessionId: string) => Partial<Pick<BotRuntimeActivity, 'turnActive' | 'pendingInteractions' | 'pendingAskUserQuestions'>> | undefined,
): BotRuntimeActivity[] {
  return runtimes.flatMap((runtime) => {
    if (runtime.botId === undefined) return [];
    const status = live(runtime.sessionId);
    return [{
      botId: runtime.botId,
      turnActive: status?.turnActive ?? runtime.turnActive,
      pendingInteractions: status?.pendingInteractions ?? runtime.pendingInteractions,
      pendingAskUserQuestions: status?.pendingAskUserQuestions ?? runtime.pendingAskUserQuestions,
    }];
  });
}

/** What a Bot is doing, from its running chats: waiting for an answer from the owner, at work, or at rest. */
export function botActivity(botId: string, runtimes: readonly BotRuntimeActivity[]): BotActivity {
  const own = runtimes.filter((runtime) => runtime.botId === botId);
  if (own.some((runtime) => runtime.pendingInteractions > 0 || runtime.pendingAskUserQuestions > 0)) return 'waiting';
  return own.some((runtime) => runtime.turnActive) ? 'working' : 'idle';
}

/** "5 minutes ago" for a time in epoch milliseconds; nothing for a number that is not a date. */
export function relativeActivity(epochMs: number, now = Date.now()): string {
  const at = new Date(epochMs);
  return Number.isFinite(at.getTime()) ? formatRelativeSessionTime(at.toISOString(), now) : '';
}

/** How something the owner asked of a Bot went: done, or why not, in words for the owner. */
export type BotOutcome = { ok: true } | { ok: false; message: string };

/** Open a chat of a Bot. A chat that cannot start is reported where the owner is, not thrown at them. */
export async function startBotChat(open: () => Promise<unknown>): Promise<BotOutcome> {
  try { await open(); return { ok: true }; }
  catch (cause) { return { ok: false, message: tr('bots_chat_start_failed', ipcErrorMessage(cause)) }; }
}

export async function removeBot(remove: () => Promise<unknown>): Promise<BotOutcome> {
  try { await remove(); return { ok: true }; }
  catch (cause) { return { ok: false, message: tr('bots_delete_failed', ipcErrorMessage(cause)) }; }
}
