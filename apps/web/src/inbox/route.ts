import { inboxKey, type InboxItem } from '../../../../packages/inbox/contracts';
import { remoteSessionRoute } from '../app/router';

export function sourceRoute(row: InboxItem): string | undefined {
  const { ref } = row;
  if (inboxKey(ref) !== row.key) throw new Error('INVALID_INBOX_REF');
  if (ref.kind === 'assistant')
    return row.remoteHostId
      ? remoteSessionRoute(row.remoteHostId, ref.conversationId) + '?inbox=' + encodeURIComponent(row.key)
      : undefined;
  if (ref.kind === 'service')
    return (
      '/services/' + encodeURIComponent(ref.sourceId) + '?session=' + encodeURIComponent(ref.conversationId)
    );
  if (row.category === 'friend' && row.peerUid) return '/friends?chat=' + encodeURIComponent(row.peerUid);
  return '/teams?' + new URLSearchParams({ team: ref.sourceId, chat: ref.conversationId });
}
