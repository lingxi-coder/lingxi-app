import type { ClientEvent, MessageDto } from '@lingxi/bridge-client';
import type { RemotePromptOrigin, RemoteRecord, RemoteSeqRecord } from '@lingxi/bridge-client/remote';
import {
  appendUserPrompt,
  conversationFromMessages,
  emptyConversation,
  isSupportedImageMediaType,
  reduceEvent,
  type ConversationState,
  type RunItem,
} from './reuse';
import { displayText } from './text';

/**
 * Prefix of an image URL that is really a reference the desktop keeps: the
 * viewer fetches the bytes with `image.get` when the user asks for them. Image
 * URLs that do not start with `data:` are treated the same way, since only the
 * desktop knows what they mean.
 */
export const IMAGE_REF_SCHEME = 'lingxi-remote-image:';

/** The reference an image URL stands for, or `undefined` for a `data:` URL that can be shown as it is. */
export function imageRefOf(url: string): string | undefined {
  if (/^data:image\/(?:png|jpeg|gif|webp);base64,/iu.test(url)) return undefined;
  return url.startsWith(IMAGE_REF_SCHEME) ? url.slice(IMAGE_REF_SCHEME.length) : url;
}

/** Where a prompt came from, for the label under a user message. */
export interface PromptMeta {
  origin: RemotePromptOrigin;
  deviceName?: string;
  turnId: number;
}

/** A transcript row the engine's event stream has no item for. */
export interface ExtraRow {
  id: string;
  kind: 'omitted' | 'computer_access';
  /** The item after which it belongs; `null` for the start of the transcript. */
  afterItemId: string | null;
  /** The apps named in a computer-access request, or the event type of an omitted event. */
  detail?: string;
}

/** Everything the transcript view needs; replaced (never mutated) on every change. */
export interface Transcript {
  conversation: ConversationState;
  promptMeta: ReadonlyMap<string, PromptMeta>;
  extras: readonly ExtraRow[];
  /** Tool calls whose result the desktop cut down before sending. */
  truncatedTools: ReadonlySet<string>;
}

export function emptyTranscript(): Transcript {
  return { conversation: emptyConversation(), promptMeta: new Map(), extras: [], truncatedTools: new Set() };
}

/**
 * Builds a transcript from a session's base conversation and the records after
 * it, and extends it one record at a time. The reducer and the message
 * conversion are the Electron renderer's, so a session reads the same here as
 * on the desktop; this class adds only what the remote stream has on top: the
 * prompt records (the engine's event stream does not contain the user's
 * prompts), image references, and rows for events too large to send.
 */
export class TranscriptBuilder {
  private conversation: ConversationState;
  private promptMeta = new Map<string, PromptMeta>();
  private extras: ExtraRow[] = [];
  private truncatedTools = new Set<string>();
  private nextExtra = 1;

  constructor(baseMessages: readonly MessageDto[] = []) {
    this.conversation = baseMessages.length > 0 ? conversationFromMessages(baseMessages) : emptyConversation();
  }

  /** A builder for `messages` with `records` applied in order. */
  static from(messages: readonly MessageDto[], records: readonly RemoteSeqRecord[]): TranscriptBuilder {
    const builder = new TranscriptBuilder(messages);
    for (const { record } of records) builder.apply(record);
    return builder;
  }

  apply(record: RemoteRecord): void {
    switch (record.type) {
      case 'event':
        this.applyEvent(record.event, record.truncated === true);
        break;
      case 'prompt':
        this.applyPrompt(record);
        break;
      case 'computer_access_request':
        this.addExtra('computer_access', displayText(record.request.apps.map((app) => app.label).join(', '), 120));
        break;
      case 'omitted':
        this.addExtra('omitted', displayText(record.event_type ?? '', 60));
        break;
      case 'permission_request':
        // What can be answered is the session's pending set; the request is not a transcript row.
        break;
    }
  }

  /** The turn state reported by the desktop wins over what the events imply. */
  setRunning(running: boolean): void {
    if (this.conversation.running !== running) this.conversation = { ...this.conversation, running };
  }

  snapshot(): Transcript {
    return {
      conversation: this.conversation,
      promptMeta: this.promptMeta,
      extras: this.extras,
      truncatedTools: this.truncatedTools,
    };
  }

  private applyEvent(event: ClientEvent, truncated: boolean): void {
    if (truncated && event.type === 'tool_use_result') {
      this.truncatedTools = new Set(this.truncatedTools).add(event.id);
    }
    this.conversation = reduceEvent(this.conversation, event);
  }

  private applyPrompt(record: Extract<RemoteRecord, { type: 'prompt' }>): void {
    const before = this.conversation;
    const images = (record.images ?? [])
      .filter((image) => isSupportedImageMediaType(image.media_type))
      .map((image) => ({ media_type: image.media_type, url: `${IMAGE_REF_SCHEME}${image.ref}` }));
    let next = appendUserPrompt(before, record.text, []);
    if (next === before) {
      if (images.length === 0) return; // nothing visible to show
      // An image-only prompt: the shared helper wants text, so build the row by hand in the same shape.
      next = {
        ...before,
        items: [
          ...before.items,
          { type: 'narration', id: `i${before.nextId}`, text: '', strong: true, role: 'user' },
        ],
        openAssistantIndex: -1,
        openThinkingIndex: -1,
        nextId: before.nextId + 1,
      };
    }
    const last = next.items[next.items.length - 1];
    if (last === undefined || last.type !== 'narration') return;
    const items = next.items.slice(0, -1);
    // Prompts have no clock of their own on the wire, so none is shown.
    items.push({ ...last, sentAt: undefined, ...(images.length > 0 ? { images } : {}) });
    this.conversation = { ...next, items };
    const meta: PromptMeta = { origin: record.origin, turnId: record.turn_id };
    const deviceName = displayText(record.device_name, 64);
    if (deviceName) meta.deviceName = deviceName;
    this.promptMeta = new Map(this.promptMeta).set(last.id, meta);
  }

  private addExtra(kind: ExtraRow['kind'], detail: string): void {
    const items = this.conversation.items;
    const row: ExtraRow = { id: `extra:${this.nextExtra++}`, kind, afterItemId: items[items.length - 1]?.id ?? null };
    if (detail) row.detail = detail;
    this.extras = [...this.extras, row];
  }
}

/**
 * The items to render: the conversation's, with a muted note inserted after the
 * item each extra row followed. `describe` supplies the (localized) text.
 */
export function itemsWithExtras(
  items: readonly RunItem[],
  extras: readonly ExtraRow[],
  describe: (extra: ExtraRow) => string,
): RunItem[] {
  if (extras.length === 0) return items.slice();
  const byAnchor = new Map<string | null, ExtraRow[]>();
  for (const extra of extras) {
    const anchor = extra.afterItemId !== null && !items.some((item) => item.id === extra.afterItemId) ? null : extra.afterItemId;
    byAnchor.set(anchor, [...(byAnchor.get(anchor) ?? []), extra]);
  }
  const note = (extra: ExtraRow): RunItem => ({ type: 'narration', id: extra.id, text: describe(extra), tone: 'muted' });
  const merged: RunItem[] = (byAnchor.get(null) ?? []).map(note);
  for (const item of items) {
    merged.push(item);
    for (const extra of byAnchor.get(item.id) ?? []) merged.push(note(extra));
  }
  return merged;
}
