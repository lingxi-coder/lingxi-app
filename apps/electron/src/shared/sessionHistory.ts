import type { MessageDto } from '@lingxi/bridge-client';

export const CH_SESSION_HISTORY = 'lingxi:session:history';
export const CH_SESSION_RESUME_PREVIEW = 'lingxi:session:resume-preview';

export interface SessionResumePreview {
  requestId: string;
  session: { projectPath: string; sessionId: string; mode: 'chat' | 'code' };
  messages: MessageDto[];
}
export interface TranscriptHistoryCursor { uuid: string; offset?: number | null; identity?: string | null }
export interface TranscriptHistoryPage { messages: MessageDto[]; before: TranscriptHistoryCursor | null }
export interface TranscriptHistoryState { before: TranscriptHistoryCursor | null; loading: boolean; error?: string }
export const EMPTY_TRANSCRIPT_HISTORY: TranscriptHistoryState = { before: null, loading: false };
const INITIAL_HISTORY_ROWS = 60;

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

/** Page the Desktop view at a persisted user turn, keeping its associated tool activity together. */
export function initialDisplayPage(messages: readonly MessageDto[]): TranscriptHistoryPage {
  const recent = recentTranscript(messages);
  let start = Math.max(0, recent.length - INITIAL_HISTORY_ROWS);
  if (start === 0) return { messages: [...recent], before: initialHistory(recent).before };
  const results = new Map<string, number>();
  // A deferred result can arrive after another prompt. Include its call if it
  // would otherwise fall on the older page.
  for (let index = recent.length - 1; index >= 0; index--) {
    const row = recent[index]!;
    for (const block of row.blocks) {
      if (block.type === 'tool_result' && !results.has(block.id)) results.set(block.id, index);
      if (block.type === 'tool_use' && (results.get(block.id) ?? -1) >= start && index < start) start = index;
    }
    if (index === start && index > 0 && (row.role !== 'user' || !row.transcript_uuid
      || !row.blocks.some(block => block.type === 'text') || row.blocks.some(block => block.type === 'tool_result'))) start--;
  }
  if (start > 0) return { messages: recent.slice(start), before: { uuid: recent[start]!.transcript_uuid! } };
  return { messages: [...recent], before: initialHistory(recent).before };
}
