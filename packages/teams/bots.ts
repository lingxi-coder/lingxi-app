import type { MessageScope, TeamMessage } from './messages';

export interface TeamBotCard {
  key: string; ownerUid: string; hostId: string; botId: string; name: string; description: string;
  revision: number; use: 'all' | 'listed'; uids: string[]; permitted: boolean; online: boolean; onlineUntil?: number; enabled?: boolean;
}
export interface LocalTeamBot { botId: string; name: string; description: string; model?: string }
export interface BindLocalTeamBotInput extends MessageScope {
  botId: string; use: 'all' | 'listed'; uids: string[]; expectedRevision: number;
}
export interface TeamBotState {
  state: string; ownerUid: string; botKey: string; origin: { uid: string; generation: number };
  leaseUntil?: number; questionId?: string; answered: boolean;
  hostStatus?: 'retrying' | 'result_pending' | 'recovery_required' | 'model_unavailable' | 'runtime_unavailable' | 'storage_error';
}
export type TeamBotAnswer = MessageScope & {
  action: 'botAnswer'; conversationId: string; triggerId: string; questionId: string;
  answerId: string; answers: Record<string, string>;
};
export type TeamBotRecovery = MessageScope & {
  action: 'recoverBot'; conversationId: string; botKey: string; triggerId: string; text: string;
};
/** Member-facing commands only. Execution and approval capabilities stay on the host. */
export type TeamBotCommand = TeamBotAnswer
  | MessageScope & { action: 'bots' }
  | MessageScope & { action: 'unbindBot'; botKey: string; expectedRevision: number }
  | MessageScope & { action: 'botState'; conversationId: string; triggerId: string; questionId?: string }
  | MessageScope & { action: 'botStates'; conversationId: string; triggerIds: string[] }
  | TeamBotRecovery;

export function isQuestionRecipient(message: TeamMessage, uid: string, generation: number): boolean {
  return message.author.kind === 'bot' && !message.recalled && Boolean(message.question && message.triggerId)
    && message.origin?.uid === uid && message.origin.generation === generation;
}
export function canRecoverBot(state: TeamBotState, uid: string, now = Date.now()): boolean {
  return state.ownerUid === uid && state.hostStatus !== 'result_pending' && (state.state === 'recovery'
    || (['claimed', 'running'].includes(state.state) && typeof state.leaseUntil === 'number' && state.leaseUntil <= now));
}
export function teamBotUsable(card: TeamBotCard): boolean { return card.enabled !== false && card.permitted; }
/** Matches the trusted backend identity; only botState confirms a trigger exists. */
export async function teamBotTriggerId(messageId: string, botKey: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(JSON.stringify([messageId, botKey])));
  return [...new Uint8Array(digest)].map(value => value.toString(16).padStart(2, '0')).join('');
}
