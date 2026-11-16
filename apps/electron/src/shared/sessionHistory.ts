import type { MessageDto } from '@lingxi/bridge-client';

export const CH_SESSION_HISTORY = 'lingxi:session:history';
export interface TranscriptHistoryCursor { uuid: string; offset?: number | null; identity?: string | null }
export interface TranscriptHistoryPage { messages: MessageDto[]; before: TranscriptHistoryCursor | null }
export interface TranscriptHistoryState { before: TranscriptHistoryCursor | null; loading: boolean; error?: string }
export const EMPTY_TRANSCRIPT_HISTORY: TranscriptHistoryState = { before: null, loading: false };

export function recentTranscript(messages: readonly MessageDto[]): readonly MessageDto[] {
  for (let index = messages.length - 1; index >= 0; index--) {
    if (messages[index]!.blocks.some(block => block.type === 'compact_boundary')) return messages.slice(index);
  }
  return messages;
}

export function historyCursor(value: unknown): TranscriptHistoryCursor {
  const input = value as TranscriptHistoryCursor;
  if (!input || typeof input !== 'object' || Array.isArray(input)
    || Object.keys(input).some(key => !['uuid', 'offset', 'identity'].includes(key))
    || typeof input.uuid !== 'string' || !/^[a-f\d]{8}(?:-[a-f\d]{4}){3}-[a-f\d]{12}$/i.test(input.uuid)
    || ((input.offset == null) !== (input.identity == null))
    || (input.offset != null && (!Number.isSafeInteger(input.offset) || input.offset < 0))
    || (input.identity != null && (typeof input.identity !== 'string' || input.identity.length > 128))) {
    throw new Error('Invalid history cursor');
  }
  return { uuid: input.uuid, ...(input.offset != null ? { offset: input.offset, identity: input.identity } : {}) };
}

/** Only a real persisted boundary can offer earlier history. */
export function initialHistory(messages: readonly MessageDto[]): TranscriptHistoryState {
  const first = messages[0];
  return { loading: false, before: first?.transcript_uuid && first.blocks.some(block => block.type === 'compact_boundary')
    ? { uuid: first.transcript_uuid } : null };
}
