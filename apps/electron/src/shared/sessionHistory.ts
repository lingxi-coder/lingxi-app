import type { MessageDto } from '@lingxi/bridge-client';
import { fileMentionsFromPrompt, fileMentionToken } from './fileMentions.js';

export const CH_SESSION_HISTORY = 'lingxi:session:history';
export const CH_SESSION_IMAGE = 'lingxi:session:history-image';
export const CH_SESSION_IMAGE_CANCEL = 'lingxi:session:history-image:cancel';
export const CH_SESSION_RESUME_PREVIEW = 'lingxi:session:resume-preview';

export type SessionImageSize = 'thumbnail' | 'original';
export interface SessionImagePayload { mediaType: string; base64: string }

export function isHistoryImageReference(value: unknown): value is string {
  return typeof value === 'string' && /^lingxi-history-image:[a-f\d]{8}(?:-[a-f\d]{4}){3}-[a-f\d]{12}:\d{1,2}:(?:png|jpeg|gif|webp)(?::\d{1,20}:[A-Za-z0-9_-]{1,172})?$/u.test(value);
}

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

/** Page at a persisted message, keeping any tool activity that spans the cut. */
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
    if (index === start && index > 0 && !row.transcript_uuid) start--;
  }
  if (start > 0) return { messages: recent.slice(start), before: { uuid: recent[start]!.transcript_uuid! } };
  return { messages: [...recent], before: initialHistory(recent).before };
}

/** Reduce rows outside the transcript window to the inputs of the overview reducers. */
export function historyOverview(messages: readonly MessageDto[]): MessageDto[] {
  return messages.flatMap(message => {
    const blocks = message.blocks.flatMap((block): MessageDto['blocks'] => {
      if (block.type === 'tool_use') {
        if (block.tool === 'ExitPlanMode' || block.tool === 'EnterPlanMode'
          || (block.tool === 'Write' && planFileContent(block.input_json))) return [block];
      }
      if (block.type === 'tool_result') {
        if (block.tool === 'ExitPlanMode') return [block];
        if (block.tool === 'Write') return [{ ...block, result_json: '', display: { body_lines: 0 } }];
      }
      if (message.role === 'user' && block.type === 'text') {
        const mentions = fileMentionsFromPrompt(block.text);
        if (mentions.length) return [{ type: 'text', text: mentions.map(fileMentionToken).join(' ') }];
      }
      return [];
    });
    if (!blocks.length && !message.images?.length) return [];
    return [{ role: message.role, transcript_uuid: message.transcript_uuid, blocks, images: message.images }];
  });
}

export function planFileContent(json: string): string | undefined {
  try {
    const input = JSON.parse(json);
    const path = typeof input?.file_path === 'string' ? input.file_path.replace(/\\/g, '/') : '';
    if (/(?:^|\/)(?:\.lingxi|\.claude)\/plans\/[^\n]+\.md$/i.test(path)
      && typeof input?.content === 'string' && input.content.trim()) return input.content;
  } catch { /* Malformed tool input does not produce a plan draft. */ }
  return undefined;
}
