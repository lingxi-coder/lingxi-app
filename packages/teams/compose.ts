import { sharedBotUsable, type SharedBotCard } from './execution.js';
import type { TeamConversation } from './messages.js';

/** The server's limit for one message body, in UTF-8 bytes. */
export const MESSAGE_BYTES = 8192;
export const byteLength = (text: string): number => new TextEncoder().encode(text).length;
/** Non-blank and within the server's limit. */
export const canSendText = (text: string): boolean => text.trim().length > 0 && byteLength(text) <= MESSAGE_BYTES;

export interface EnterKey { key: string; shiftKey: boolean; isComposing?: boolean; keyCode?: number }
/**
 * Enter sends and Shift+Enter is a newline. Enter that confirms an input-method candidate
 * (`isComposing`, or the legacy keyCode 229 some browsers report) does neither.
 */
export function sendsOnEnter(event: EnterKey): boolean {
  return event.key === 'Enter' && !event.shiftKey && !event.isComposing && event.keyCode !== 229;
}

/** The `@query` being typed right before the caret. `start` is the index of the `@`. */
export function mentionAt(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const match = /(^|[\s(（])@([^\s@]{0,40})$/u.exec(before);
  if (!match) return null;
  const query = match[2]!;
  return { start: before.length - query.length - 1, query };
}

/** The text without the `@query` that ran from `start` to `caret`, and without the double space it leaves. */
export function withoutMention(text: string, start: number, caret: number): string {
  const before = text.slice(0, start), after = text.slice(caret);
  return before.endsWith(' ') && after.startsWith(' ') ? before + after.slice(1) : before + after;
}

/**
 * Bots that can be asked in a conversation: usable now, taking part in it, not chosen yet, and
 * matching what was typed. Only groups and channels take explicit targets; a Bot DM asks its Bot
 * automatically and a person-to-person DM has none.
 */
export function mentionCandidates(bots: readonly SharedBotCard[], conversation: Pick<TeamConversation, 'kind' | 'participants'>, chosen: readonly string[], query: string): SharedBotCard[] {
  if (conversation.kind !== 'group' && conversation.kind !== 'channel') return [];
  const needle = query.trim().toLowerCase();
  return bots.filter(bot => sharedBotUsable(bot) && conversation.participants.includes(`shared_bot:${bot.sharedBotId}`) && !chosen.includes(bot.sharedBotId) && (!needle || bot.name.toLowerCase().includes(needle)));
}

/** One line of a message for a quote or a reply bar: whitespace collapsed, cut at `length` characters. */
export function snippet(text: string, length = 90): string {
  const chars = Array.from(text.replace(/\s+/gu, ' ').trim());
  return chars.length > length ? `${chars.slice(0, length).join('')}…` : chars.join('');
}
