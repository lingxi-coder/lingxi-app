export const INBOX_PAGE_SIZE = 50;
export const INBOX_SCHEMA_VERSION = 1;
export type InboxKind = 'assistant' | 'team' | 'service';
export type InboxCategory = InboxKind | 'friend';
export type NotificationMode = 'all' | 'mentions' | 'none';
export interface InboxRef {
  kind: InboxKind;
  sourceId: string;
  conversationId: string;
  generation: number;
}
export interface InboxPreferences {
  pinOrder: number | null;
  notificationMode: NotificationMode;
  archived: boolean;
}
export interface InboxItem extends InboxPreferences {
  schemaVersion: 1;
  key: string;
  ref: InboxRef;
  category: InboxCategory;
  peerUid?: string;
  /** A team row's conversation kind; absent on rows folded before it existed. */
  conversationKind?: 'dm' | 'group' | 'channel' | 'bot_dm';
  sourceLabel?: string;
  remoteHostId?: string;
  title: string;
  preview: string;
  lastActivityAt: number;
  sourceCursor: number;
  sourceRevision: number;
  readThrough: number;
  unreadCount: number;
  mentionCount: number;
  pendingActionCount: number;
  pendingActionAt?: number;
  pendingActionRevision?: number;
  failureRevision?: number;
  failureAt?: number;
  attentionKind?: string;
  state: string;
  manualUnread: boolean;
  manualUnreadRevision: number;
  stateRevision: number;
  preferencesRevision?: number;
  offline: boolean;
}
export interface InboxCursor {
  titleSort?: string;
  pinOrder?: number | null;
  lastActivityAt: number;
  key: string;
}
export interface InboxNoticeTarget {
  through: number;
  messageId?: string;
  taskId?: string;
  key?: string;
  generation?: number;
}
export interface InboxNotice extends InboxNoticeTarget {
  id: string;
  key: string;
  generation: number;
  kind: string;
  lastActivityAt: number;
  deliverAfter: number;
  deliverable: boolean;
}
export interface InboxPage {
  cloudUnavailable?: boolean;
  partialOffline?: boolean;
  items: InboxItem[];
  next?: InboxCursor | null;
}
export interface FrozenRead {
  observedPendingActionRevision?: number;
  observedFailureRevision?: number;
  operationId?: string;
  key: string;
  generation: number;
  through: number;
  manualUnreadRevision: number;
}
export interface InboxReadBatch {
  jobId: string;
  state: 'queued' | 'running' | 'complete' | 'expired';
  processed?: number;
  changed?: number;
}
export type InboxMutation =
  | ({ action: 'markRead' } & FrozenRead)
  | {
      action: 'markUnread';
      key: string;
      generation: number;
      operationId: string;
      expectedManualUnreadRevision?: number;
    }
  | {
      action: 'preferences';
      key: string;
      generation: number;
      expectedPreferencesRevision?: number;
      patch: Partial<InboxPreferences>;
    }
  | { action: 'markAllRead'; entries: FrozenRead[] };
export type InboxFilter = 'all' | 'unread' | 'attention';
export type InboxCommand =
  | InboxMutation
  | {
      action: 'markAllReadSnapshot';
      operationId: string;
      requestedAt: number;
      category?: InboxCategory;
      filter?: InboxFilter;
      archived?: boolean;
      searchPrefix?: string;
    }
  | { action: 'readBatchStatus'; jobId: string }
  | {
      action: 'list';
      searchPrefix?: string;
      after?: InboxCursor;
      filter?: InboxFilter;
      category?: InboxCategory;
      archived?: boolean;
    }
  | {
      action: 'assistantRemove';
      conversationConsentRevision?: number;
      sourceId: string;
      sourceSecret: string;
      conversationId: string;
      generation: number;
      sourceRevision: number;
    }
  | {
      action: 'assistantSource';
      sourceId: string;
      sourceSecret: string;
      enabled: boolean;
      consentRevision?: number;
    }
  | {
      action: 'assistantPublish';
      conversationConsentRevision?: number;
      failureAt?: number;
      failureRevision?: number;
      sourceId: string;
      sourceSecret: string;
      conversationId: string;
      generation: number;
      baseline: number;
      sourceRevision: number;
      sourceCursor: number;
      lastActivityAt: number;
      state:
        | 'idle'
        | 'running'
        | 'completed'
        | 'failed'
        | 'waiting_for_input'
        | 'waiting_for_approval'
        | 'offline';
      pendingActionCount: number;
      pendingActionAt?: number;
      pendingActionRevision?: number;
      title?: string;
      sourceLabel?: string;
      remoteHostId?: string;
      previewKind: 'reply' | 'taskResult' | 'failure' | null;
    }
  | { action: 'accountPreferences'; dmReceipts?: boolean }
  | { action: 'peerReceipt'; key: string; generation: number }
  | { action: 'presence'; deviceId: string; key: string | null; generation?: number; foreground: boolean }
  | { action: 'get'; key: string; generation?: number }
  | { action: 'notice'; id: string; deviceId?: string }
  | { action: 'notices'; after?: InboxCursor; deviceId?: string };
export type InboxRequest = <T>(command: InboxCommand) => Promise<T>;
export function inboxKey(ref: Pick<InboxRef, 'kind' | 'sourceId' | 'conversationId'>): string {
  if (
    !['team', 'assistant', 'service'].includes(ref.kind) ||
    ![ref.sourceId, ref.conversationId].every((id) => /^[A-Za-z0-9_-]{1,128}$/u.test(id))
  )
    throw new Error('INVALID_INBOX_REF');
  return [ref.kind, ref.sourceId, ref.conversationId].join(':');
}
export function freezeRead(item: InboxItem, through = item.sourceCursor): FrozenRead {
  return {
    operationId: crypto.randomUUID(),
    key: item.key,
    generation: item.ref.generation,
    through,
    ...(item.failureRevision === undefined ? {} : { observedFailureRevision: item.failureRevision }),
    ...(item.pendingActionRevision === undefined
      ? {}
      : { observedPendingActionRevision: item.pendingActionRevision }),
    manualUnreadRevision: item.manualUnreadRevision,
  };
}
export const isUnread = (item: InboxItem) => item.manualUnread || item.unreadCount > 0;
export const needsAttention = (item: InboxItem) =>
  item.pendingActionCount > 0 || item.mentionCount > 0 || !!item.attentionKind;
export function orderItems(a: InboxItem, b: InboxItem) {
  return (
    (a.pinOrder ?? Infinity) - (b.pinOrder ?? Infinity) ||
    b.lastActivityAt - a.lastActivityAt ||
    (a.key < b.key ? -1 : a.key > b.key ? 1 : 0)
  );
}
