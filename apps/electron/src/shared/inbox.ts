import type { InboxCommand } from '../../../../packages/inbox/contracts.js';
import type { SessionRef } from './settings.js';

export const CH_INBOX = 'lingxi:inbox';
export const CH_INBOX_CHANGED = 'lingxi:inbox:changed';
export const CH_INBOX_NOTIFICATION = 'lingxi:inbox:notification';
export type InboxSignal = { uid: string | null; type: 'dirty' | 'badge' | 'error'; count?: number; error?: string };
export interface InboxNotification {
  title: string;
  body: string;
  ref?: SessionRef;
  key?: string;
  generation?: number;
  activate?: boolean;
  through?: number;
  messageId?: string;
  taskId?: string;
}
export interface AssistantActivityPage {
  items: { cursor: number; resultUuids: string[]; kind: string; at: number }[];
  cursor: number;
  baselineCursor: number;
  hasMore: boolean;
}
export interface AssistantActivityPreview {
  available: boolean;
  messages: import('@lingxi/bridge-client').MessageDto[];
}
export interface DesktopInboxApi {
  activityPreview(key: string, through: number): Promise<AssistantActivityPreview>;
  activityPage(key: string, after: number): Promise<AssistantActivityPage>;
  request<T>(uid: string | null, command: InboxCommand): Promise<T>;
  watch(
    uid: string | null,
    dirty: () => void,
    badge: (count: number) => void,
    error: (error: Error) => void,
  ): () => void;
  setVisible(sessionId: string | null, key?: string | null, generation?: number): Promise<void>;
  onNotification(fn: (notice: InboxNotification) => void): () => void;
  localRef(key: string): Promise<SessionRef | null>;
  conversationSummarySync(uid: string, key: string): Promise<boolean>;
  setConversationSummarySync(uid: string, key: string, enabled: boolean): Promise<void>;
  summarySync(uid: string): Promise<boolean>;
  setSummarySync(uid: string, enabled: boolean): Promise<void>;
}
