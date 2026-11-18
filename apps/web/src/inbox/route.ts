import { inboxKey, type InboxItem, type InboxNoticeTarget } from '../../../../packages/inbox/contracts';
import { remoteSessionRoute } from '../app/router';
const safe = /^[A-Za-z0-9_-]{1,128}$/u;
export function noticeTargetFromSearch(search: string): InboxNoticeTarget | undefined {
  const params = new URLSearchParams(search);
  if (!params.has('through') && !params.has('preview')) return;
  const raw = params.get('through'),
    through = raw && /^\d+$/u.test(raw) ? Number(raw) : -1;
  const messageId = params.get('messageId') ?? params.get('message'),
    taskId = params.get('taskId') ?? params.get('task');
  const gen = params.get('generation'),
    key = params.get('inbox');
  return {
    through:
      Number.isSafeInteger(through) && (!messageId || safe.test(messageId)) && (!taskId || safe.test(taskId))
        ? through
        : -1,
    ...(messageId && safe.test(messageId) ? { messageId } : {}),
    ...(taskId && safe.test(taskId) ? { taskId } : {}),
    ...(gen ? { generation: Number(gen) } : {}),
    ...(key ? { key } : {}),
  };
}
export function sourceRoute(row: InboxItem, target?: InboxNoticeTarget): string | undefined {
  const { ref } = row;
  if (
    inboxKey(ref) !== row.key ||
    (target?.key && target.key !== row.key) ||
    (target?.generation !== undefined && target.generation !== ref.generation)
  )
    throw new Error('INVALID_INBOX_REF');
  let path: string;
  const params = new URLSearchParams();
  if (ref.kind === 'assistant') {
    if (!row.remoteHostId) return;
    path = remoteSessionRoute(row.remoteHostId, ref.conversationId);
    params.set('inbox', row.key);
  } else if (ref.kind === 'service') {
    path = '/services/' + encodeURIComponent(ref.sourceId);
    params.set('session', ref.conversationId);
  } else if (row.category === 'friend' && row.peerUid) {
    path = '/friends';
    params.set('chat', row.peerUid);
  } else {
    path = '/teams';
    params.set('team', ref.sourceId);
    params.set('chat', ref.conversationId);
  }
  if (target) {
    params.set('preview', '1');
    params.set('through', String(target.through));
    params.set('inbox', row.key);
    params.set('generation', String(ref.generation));
    if (target.messageId) params.set('message', target.messageId);
    if (target.taskId) params.set('task', target.taskId);
  }
  return path + '?' + params;
}
