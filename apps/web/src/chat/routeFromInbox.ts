import type { InboxItem } from '../../../../packages/inbox/contracts';
import type { ChatTarget } from '../app/router';
import { sourceRoute } from '../inbox/route';

/** The sidebar section a conversation belongs to. */
export type ChatSection = 'teams' | 'friends' | 'bots' | 'computers';

/** Where an inbox row opens: a place inside the Chat workspace, or another page (Remote Control, a Bot service). */
export type RowDestination = { kind: 'chat'; target: ChatTarget } | { kind: 'page'; href: string };

export function sectionOf(row: InboxItem): ChatSection {
  if (row.ref.kind === 'assistant') return 'computers';
  if (row.ref.kind === 'service' || row.conversationKind === 'bot_dm') return 'bots';
  return row.category === 'friend' ? 'friends' : 'teams';
}

export function destinationOf(row: InboxItem): RowDestination | undefined {
  const { ref } = row;
  if (ref.kind === 'team') {
    if (row.category === 'friend' && row.peerUid) return { kind: 'chat', target: { kind: 'friend', uid: row.peerUid } };
    return { kind: 'chat', target: { kind: 'team', teamId: ref.sourceId, conversationId: ref.conversationId } };
  }
  try {
    const href = sourceRoute(row);
    return href ? { kind: 'page', href } : undefined;
  } catch {
    return undefined;
  }
}

/** Whether a row is the conversation that is open. */
export function isOpen(row: InboxItem, target: ChatTarget): boolean {
  const destination = destinationOf(row);
  if (destination?.kind !== 'chat') return false;
  const open = destination.target;
  if (open.kind === 'friend') return target.kind === 'friend' && target.uid === open.uid;
  return open.kind === 'team' && target.kind === 'team' && target.teamId === open.teamId && target.conversationId === open.conversationId;
}
