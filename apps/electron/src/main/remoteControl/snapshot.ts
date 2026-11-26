/**
 * What a viewer is shown of a session, as pure functions of a runtime's
 * {@link RemoteRuntimeView}: the snapshot it starts from, the records that
 * follow, and the single records of the live stream.
 *
 * The rules, in the order they are applied to the records after the base:
 * merge the runtime's retained events, latest state, prompt journal and
 * interaction records by sequence; keep what the remote disposition table
 * lets through (default deny: an event the table does not know is withheld);
 * merge adjacent deltas and take the `signature` off thinking; cut the
 * tool-call fields above 64 KiB (and say so); replace an event above 1 MiB by
 * an `omitted` record; turn images into references the viewer fetches with
 * `image.get`; split into pages of at most 256 KiB.
 */

import type { ClientEvent, MessageBlockDto, MessageDto } from '@lingxi/bridge-client';
import type {
  RemoteImageRef,
  RemoteRecord,
  RemoteSeqRecord,
  SessionAttachParams,
  SessionHistoryResult,
  SessionSnapshotParams,
} from '@lingxi/bridge-client/remote';
import { MESSAGE_MAX_BYTES } from '@lingxi/bridge-client/remote';

import { CLIENT_EVENT_REMOTE_DISPOSITIONS, type RemoteEventDisposition } from '../../shared/clientCommands.js';
import { isHistoryImageReference } from '../../shared/sessionHistory.js';
import type { InteractionRecord, PromptJournalEntry, RemoteRuntimeView, SequencedRuntimeEventEnvelope } from '../bridgeTypes.js';

/** One page of messages or records. */
export const PAGE_MAX_BYTES = 256 * 1024;
/** JSON byte budget for the initial page. Larger messages remain pageable. */
export const INITIAL_HISTORY_MAX_BYTES = 4 * 1024 * 1024;
/** Maximum number of base messages projected for the first screen. */
export const INITIAL_HISTORY_TAIL_MESSAGES = 60;
/** `input_json` and `result_json` longer than this are cut. */
export const TOOL_FIELD_MAX_BYTES = 64 * 1024;
/** An event larger than this is not sent. */
export const EVENT_MAX_BYTES = 1024 * 1024;

type Message = MessageDto;

const jsonBytes = (value: unknown): number => Buffer.byteLength(JSON.stringify(value), 'utf8');

/** Leave room before the live base for older pages without changing the wire cursor. */
export function remoteBaseOffset(view: RemoteRuntimeView): number {
  return view.historyBefore ? 1_000_000_000 : 0;
}

export function eventDisposition(type: string): RemoteEventDisposition {
  return (CLIENT_EVENT_REMOTE_DISPOSITIONS as Record<string, RemoteEventDisposition | undefined>)[type] ?? 'withheld';
}

// ─────────────────────────────────────────────────────────────────────────────
// Images
// ─────────────────────────────────────────────────────────────────────────────

// A reference names its session, because `image.get` does not.

/** Reference of the `index`-th image of the prompt journaled at `sequence`. */
export const promptImageRef = (sessionId: string, sequence: number, index: number): string => `${sessionId}:p:${sequence}:${index}`;
/** Reference of the `index`-th image of base message `messageIndex`. */
export const baseImageRef = (sessionId: string, messageIndex: number, index: number): string => `${sessionId}:m:${messageIndex}:${index}`;
/** Reference of the `index`-th image of the message carried by the event at `sequence`. */
export const eventImageRef = (sessionId: string, sequence: number, index: number): string => `${sessionId}:e:${sequence}:${index}`;

export type ParsedImageRef = { sessionId: string } & (
  | { kind: 'prompt'; sequence: number; index: number }
  | { kind: 'base'; message: number; index: number }
  | { kind: 'event'; sequence: number; index: number }
  | { kind: 'history'; token: string; signature: string }
);

export function parseImageRef(ref: string): ParsedImageRef | undefined {
  const history = /^([0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}):h:([A-Za-z0-9_-]{1,360}):([A-Za-z0-9_-]{22})$/u.exec(ref);
  if (history) return { sessionId: history[1]!, kind: 'history', token: history[2]!, signature: history[3]! };
  const match = /^([0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}):([pme]):(\d{1,16}):(\d{1,4})$/i.exec(ref);
  if (!match) return undefined;
  const sessionId = match[1]!;
  const first = Number(match[3]);
  const index = Number(match[4]);
  if (!Number.isSafeInteger(first)) return undefined;
  switch (match[2]) {
    case 'p': return { sessionId, kind: 'prompt', sequence: first, index };
    case 'm': return { sessionId, kind: 'base', message: first, index };
    default: return { sessionId, kind: 'event', sequence: first, index };
  }
}

/** The media type and base64 of a `data:` URL; anything else is not served. */
export function parseDataUrl(url: string): { media_type: string; base64: string } | undefined {
  const match = /^data:(image\/(?:png|jpeg|gif|webp));base64,([A-Za-z0-9+/]+={0,2})$/u.exec(url);
  return match ? { media_type: match[1]!, base64: match[2]! } : undefined;
}

// ─────────────────────────────────────────────────────────────────────────────
// Messages
// ─────────────────────────────────────────────────────────────────────────────

/** A string cut to at most `max` UTF-8 bytes, never in the middle of a character. */
function cutText(text: string, max: number): string {
  if (Buffer.byteLength(text, 'utf8') <= max) return text;
  const bytes = Buffer.from(text, 'utf8');
  let end = max;
  while (end > 0 && (bytes[end]! & 0xc0) === 0x80) end--;
  return bytes.subarray(0, end).toString('utf8');
}

interface Cut { value: string; cut: boolean }

function cutField(text: string): Cut {
  const value = cutText(text, TOOL_FIELD_MAX_BYTES);
  return { value, cut: value.length !== text.length };
}

function mapBlock(block: MessageBlockDto, cuts: { any: boolean }): MessageBlockDto {
  switch (block.type) {
    case 'thinking': {
      const { signature: _signature, ...rest } = block;
      return rest;
    }
    case 'tool_use': {
      const input = cutField(block.input_json);
      if (input.cut) { cuts.any = true; return { ...block, input_json: input.value }; }
      return block;
    }
    case 'tool_result': {
      const result = cutField(block.result_json);
      if (result.cut) { cuts.any = true; return { ...block, result_json: result.value }; }
      return block;
    }
    default:
      return block;
  }
}

/**
 * A message as a viewer receives it: thinking without its signature, tool
 * fields cut, and locally resolvable images represented as remote references.
 */
export function mapMessage(message: Message, refFor: (index: number) => string, cuts: { any: boolean }): Message {
  const mapped: Message = { ...message, blocks: message.blocks.map((block) => mapBlock(block, cuts)) };
  if (message.images !== undefined) {
    const images = message.images.flatMap((image, index) => {
      const url = (isHistoryImageReference(image.url) || parseDataUrl(image.url)) && refFor(index);
      return url ? [{ media_type: image.media_type, url }] : [];
    });
    if (images.length > 0) mapped.images = images;
    else delete mapped.images;
  }
  return mapped;
}

// ─────────────────────────────────────────────────────────────────────────────
// Records
// ─────────────────────────────────────────────────────────────────────────────

/** The event as a viewer receives it, and whether a field was cut. */
function mapEvent(sessionId: string, event: ClientEvent, sequence: number): { event: ClientEvent; truncated: boolean } {
  switch (event.type) {
    case 'thinking_delta': {
      const { signature: _signature, ...rest } = event;
      return { event: rest, truncated: false };
    }
    case 'tool_use_started': {
      const input = cutField(event.input_json);
      return input.cut ? { event: { ...event, input_json: input.value }, truncated: true } : { event, truncated: false };
    }
    case 'tool_use_result': {
      const result = cutField(event.result_json);
      return result.cut ? { event: { ...event, result_json: result.value }, truncated: true } : { event, truncated: false };
    }
    case 'message_complete': {
      if (event.message === undefined) return { event, truncated: false };
      const cuts = { any: false };
      const message = mapMessage(event.message, (index) => eventImageRef(sessionId, sequence, index), cuts);
      return { event: { ...event, message }, truncated: cuts.any };
    }
    default:
      return { event, truncated: false };
  }
}

function omitted(event: { type: string }, size: number): RemoteRecord {
  return { type: 'omitted', event_type: event.type, size_bytes: size };
}

/** The record for a retained engine event, or nothing when the table holds it back. */
export function recordForEvent(envelope: SequencedRuntimeEventEnvelope<ClientEvent>): RemoteSeqRecord | undefined {
  const disposition = eventDisposition(envelope.event.type);
  if (disposition === 'withheld' || disposition === 'drop') return undefined;
  // The base events are the snapshot's starting point, not records.
  if (envelope.event.type === 'session_started' || envelope.event.type === 'session_resumed') return undefined;
  const { event, truncated } = mapEvent(envelope.sessionId, envelope.event, envelope.sequence);
  const record: RemoteRecord = { type: 'event', event, ...(truncated ? { truncated: true } : {}) };
  const size = jsonBytes(record);
  return { seq: envelope.sequence, record: size > EVENT_MAX_BYTES ? omitted(event, size) : record };
}

/**
 * A prompt as a viewer sees it. A task another Bot handed over has the origin
 * `bot` and the sender's name, but only for a viewer that understands them: to
 * any other it is a `scheduled` prompt, as it was before Bots could hand over tasks.
 */
export function recordForPrompt(sessionId: string, entry: PromptJournalEntry, botAware = false): RemoteSeqRecord {
  const images: RemoteImageRef[] = entry.images.flatMap((image, index) => (
    image.base64 === undefined ? [] : [{ ref: promptImageRef(sessionId, entry.sequence, index), media_type: image.media_type }]
  ));
  return {
    seq: entry.sequence,
    record: {
      type: 'prompt',
      turn_id: entry.turnId,
      text: entry.text,
      ...(images.length > 0 ? { images } : {}),
      origin: entry.origin === 'bot' && !botAware ? 'scheduled' : entry.origin,
      ...(entry.deviceName !== undefined ? { device_name: entry.deviceName } : {}),
      ...(botAware && entry.fromBotName !== undefined ? { from_bot_name: entry.fromBotName } : {}),
    },
  };
}

export function recordForInteraction(interaction: InteractionRecord): RemoteSeqRecord {
  const size = jsonBytes(interaction.record);
  return {
    seq: interaction.sequence,
    record: size > EVENT_MAX_BYTES ? { type: 'omitted', event_type: interaction.record.type, size_bytes: size } : interaction.record,
  };
}

/**
 * Merge adjacent deltas of one kind into one record at the later sequence.
 * "Adjacent" is in the stream a viewer sees: records the table held back do
 * not separate two deltas.
 */
export function coalesceRecords(records: readonly RemoteSeqRecord[]): RemoteSeqRecord[] {
  const out: RemoteSeqRecord[] = [];
  for (const item of records) {
    const previous = out[out.length - 1];
    const record = item.record;
    if (previous !== undefined && previous.record.type === 'event' && record.type === 'event') {
      const before = previous.record.event;
      const event = record.event;
      if (before.type === 'text_delta' && event.type === 'text_delta') {
        out[out.length - 1] = { seq: item.seq, record: { type: 'event', event: { type: 'text_delta', text: before.text + event.text } } };
        continue;
      }
      if (before.type === 'thinking_delta' && event.type === 'thinking_delta') {
        out[out.length - 1] = { seq: item.seq, record: { type: 'event', event: { type: 'thinking_delta', thinking: before.thinking + event.thinking } } };
        continue;
      }
    }
    out.push(item);
  }
  return out;
}

/**
 * A delta requires the current base and a cursor inside the retained sequence range.
 */
export function canResume(
  view: Pick<RemoteRuntimeView, 'epoch' | 'baseSequence' | 'lastSequence' | 'droppedThroughSequence'>,
  resume: SessionAttachParams['resume'],
): resume is NonNullable<SessionAttachParams['resume']> {
  return resume !== undefined
    && resume.epoch === view.epoch
    && Number.isSafeInteger(resume.after_seq)
    && resume.after_seq >= view.baseSequence
    && resume.after_seq <= view.lastSequence
    && resume.after_seq >= view.droppedThroughSequence;
}

/**
 * The records after `afterSeq`: what a viewer that holds everything up to it
 * is missing. For a snapshot `afterSeq` is the base's sequence, and the
 * latest state is sent whatever its sequence (`allState`): a model chosen
 * before the base is still the model.
 */
export function recordsAfter(view: RemoteRuntimeView, afterSeq: number, allState = false, botAware = false): RemoteSeqRecord[] {
  if (!allState && afterSeq === view.lastSequence) return [];
  const items: RemoteSeqRecord[] = [];
  const seen = new Set<number>();
  const add = (item: RemoteSeqRecord | undefined, any = false): void => {
    if (item !== undefined && (any || item.seq > afterSeq) && !seen.has(item.seq)) { seen.add(item.seq); items.push(item); }
  };
  for (const envelope of view.events) add(envelope.sequence > afterSeq ? recordForEvent(envelope) : undefined);
  for (const envelope of view.state) add(allState || envelope.sequence > afterSeq ? recordForEvent(envelope) : undefined, allState);
  for (const entry of view.prompts) add(entry.sequence > afterSeq ? recordForPrompt(view.sessionId, entry, botAware) : undefined);
  for (const interaction of view.interactions) add(interaction.sequence > afterSeq ? recordForInteraction(interaction) : undefined);
  items.sort((a, b) => a.seq - b.seq);
  return coalesceRecords(items);
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot
// ─────────────────────────────────────────────────────────────────────────────

/** Items grouped into pages of at most `limit` bytes of JSON; an item larger than that gets a page of its own. */
export function pageBySize<T>(items: readonly T[], limit = PAGE_MAX_BYTES, itemBytes: (item: T, index: number) => number = jsonBytes): T[][] {
  const pages: T[][] = [];
  let page: T[] = [];
  let size = 0;
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index]!;
    const bytes = itemBytes(item, index);
    if (page.length > 0 && size + bytes > limit) { pages.push(page); page = []; size = 0; }
    page.push(item);
    size += bytes;
  }
  if (page.length > 0) pages.push(page);
  return pages;
}

function snapshotMessages(view: RemoteRuntimeView): { first: number; pages: Message[][] } {
  const base = view.base?.event;
  if (base?.type !== 'session_resumed') return { first: 0, pages: [] };

  const firstTail = Math.max(0, base.messages.length - INITIAL_HISTORY_TAIL_MESSAGES);
  const offset = remoteBaseOffset(view);
  const messages: Message[] = [];
  const sizes: number[] = [];
  const cuts = { any: false };
  let arrayBytes = 2; // Opening and closing brackets.
  for (let index = base.messages.length - 1; index >= firstTail; index -= 1) {
    const message = mapMessage(base.messages[index]!, image => baseImageRef(view.sessionId, offset + index, image), cuts);
    const bytes = jsonBytes(message);
    const nextBytes = arrayBytes + bytes + (messages.length > 0 ? 1 : 0);
    if (nextBytes > INITIAL_HISTORY_MAX_BYTES) break;
    arrayBytes = nextBytes;
    messages.push(message);
    sizes.push(bytes);
  }
  sizes.reverse();
  return {
    first: offset + base.messages.length - messages.length,
    pages: pageBySize(messages.reverse(), PAGE_MAX_BYTES, (_message, index) => sizes[index]!),
  };
}

/** A snapshot as the frames it is sent in, in order: `begin`, the `messages` pages, the `records` pages, `end`. */
export function buildSnapshot(view: RemoteRuntimeView, botAware = false): SessionSnapshotParams[] {
  const sessionId = view.sessionId;
  const { first, pages } = snapshotMessages(view);
  const mode = view.sessionMode;
  const frames: SessionSnapshotParams[] = [{
    phase: 'begin',
    session_id: sessionId,
    epoch: view.epoch,
    ...(mode !== undefined ? { session_mode: mode } : {}),
    first_message_index: first,
  }];
  for (const messages of pages) frames.push({ phase: 'messages', session_id: sessionId, messages });
  for (const records of pageBySize(recordsAfter(view, view.baseSequence, true, botAware))) frames.push({ phase: 'records', session_id: sessionId, records });
  frames.push({
    phase: 'end',
    session_id: sessionId,
    cursor: { epoch: view.epoch, seq: view.lastSequence },
    pending: view.pending,
    turn: view.turn,
    ...(view.controls !== undefined ? { controls: view.controls } : {}),
  });
  return frames;
}

/** `session.history`: up to `limit` base messages that end just before `beforeIndex`. */
export function historyPage(view: RemoteRuntimeView, beforeIndex: number, limit: number): SessionHistoryResult {
  const base = view.base?.event;
  const source = base?.type === 'session_resumed' ? base.messages : [];
  const offset = remoteBaseOffset(view);
  const end = Math.max(0, Math.min(beforeIndex - offset, source.length));
  const minimum = Math.max(0, end - limit);
  const messages = historyTail(source, end, minimum, (row, image) => baseImageRef(view.sessionId, offset + row, image));
  const start = offset + end - messages.length;
  return { messages, first_index: start, has_more: start > 0 };
}

/** Keep the next cursor reachable even when a single message exceeds one page. */
export function historyTail(source: readonly Message[], end: number, minimum: number,
  imageRef: (row: number, image: number) => string): Message[] {
  const messages: Message[] = [];
  const cuts = { any: false };
  let bytes = 2;
  for (let row = end - 1; row >= minimum; row--) {
    const message = mapMessage(source[row]!, image => imageRef(row, image), cuts);
    const size = jsonBytes(message) + (messages.length ? 1 : 0);
    if (bytes + size > INITIAL_HISTORY_MAX_BYTES) {
      if (messages.length) break;
      // Allow a single larger row if it can still fit the RPC envelope.
      messages.push(size < MESSAGE_MAX_BYTES - 1024 ? message : {
        role: message.role, transcript_uuid: message.transcript_uuid,
        blocks: [{ type: 'text', text: 'This message exceeds the remote display limit. Open the chat on its Host to view it.' }],
      });
      break;
    }
    messages.push(message);
    bytes += size;
  }
  return messages.reverse();
}

function localImage(url: string | undefined) {
  if (url === undefined) return undefined;
  return isHistoryImageReference(url) ? { historyReference: url } : parseDataUrl(url);
}

/** Resolve a remote reference against the runtime's retained messages. */
export function resolveImage(
  view: RemoteRuntimeView,
  ref: string,
  promptImage: (sequence: number, index: number) => { media_type: string; base64: string } | undefined,
): { media_type: string; base64: string } | { historyReference: string } | undefined {
  const parsed = parseImageRef(ref);
  if (parsed === undefined || parsed.sessionId !== view.sessionId) return undefined;
  switch (parsed.kind) {
    case 'history':
      return undefined; // Archive tokens are verified by RemoteCommands.
    case 'prompt':
      return promptImage(parsed.sequence, parsed.index);
    case 'base': {
      const base = view.base?.event;
      const index = parsed.message - remoteBaseOffset(view);
      const url = base?.type === 'session_resumed' ? base.messages[index]?.images?.[parsed.index]?.url : undefined;
      return localImage(url);
    }
    case 'event': {
      const envelope = view.events.find((candidate) => candidate.sequence === parsed.sequence);
      const event = envelope?.event;
      const url = event?.type === 'message_complete' ? event.message?.images?.[parsed.index]?.url : undefined;
      return localImage(url);
    }
  }
}
