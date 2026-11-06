import { botDraftProblems, emptyBotDraft, hasBotDraftProblems, type BotDraft } from './botForm';
import type { SessionRef } from '../../shared/settings';

export const BOT_PROPOSAL_LANGUAGE = 'lingxi-bot';
export const BOT_CREATION_REQUEST_MAX = 8_000;

/** A proposal is text only. Access settings always come from the native form. */
export interface BotCreationProposal {
  schemaVersion: 1;
  name: string;
  description: string;
  persona: string;
}

export function draftFromProposal(proposal: BotCreationProposal): BotDraft {
  return { ...emptyBotDraft(), name: proposal.name, description: proposal.description, persona: proposal.persona };
}

export function parseBotProposal(source: string): BotCreationProposal | undefined {
  if (source.length > 100_000) return;
  try {
    const value: unknown = JSON.parse(source);
    if (!value || typeof value !== 'object' || Array.isArray(value)) return;
    const proposal = value as Record<string, unknown>;
    if (Object.keys(proposal).some(key => !['schemaVersion', 'name', 'description', 'persona'].includes(key))
      || proposal.schemaVersion !== 1 || typeof proposal.name !== 'string'
      || typeof proposal.description !== 'string' || typeof proposal.persona !== 'string'
      || !proposal.persona.trim() || proposal.persona.includes('\0')) return;
    const parsed: BotCreationProposal = { schemaVersion: 1, name: proposal.name.trim(), description: proposal.description.trim(), persona: proposal.persona };
    return hasBotDraftProblems(botDraftProblems(draftFromProposal(parsed))) ? undefined : parsed;
  } catch { return undefined; }
}

export function botCreationPrompt(request: string): string {
  const text = request.trim();
  if (!text || text.length > BOT_CREATION_REQUEST_MAX) throw new Error('invalid Bot creation request');
  return `/create-bot ${text}`;
}

/** Prepare a new chat without submitting into an existing conversation. */
export async function prepareBotCreationChat(request: string, navigation: {
  newSession(purpose: { kind: 'create-bot' }): Promise<SessionRef | undefined>;
  offerLinkDraft(draft: { sessionId: string; text: string; requestId: string }): void;
}, isCurrent: () => boolean): Promise<SessionRef | undefined> {
  const text = botCreationPrompt(request);
  if (!isCurrent()) return;
  const session = await navigation.newSession({ kind: 'create-bot' });
  if (!session || !isCurrent()) return;
  navigation.offerLinkDraft({ sessionId: session.sessionId, text, requestId: `create-bot:${session.sessionId}` });
  return session;
}
