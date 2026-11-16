import type { ClientEvent } from '@lingxi/bridge-client';
import type { SequencedRuntimeEventEnvelope } from './bridgeTypes.js';

type Envelope = SequencedRuntimeEventEnvelope<ClientEvent>;
interface Entry { envelope: Envelope; previous?: Entry; next?: Entry }

/** Raw recent frames remain available for ordinary remote cursor resumes. */
export const REPLAY_LIVE_TAIL_LIMIT = 256;

/** State that a source transcript does not carry, restored after a new base. */
export function replayCheckpointState(events: readonly Envelope[]): ClientEvent[] {
  const latest = new Map<string, Envelope>();
  let usage: Envelope & { event: Extract<ClientEvent, { type: 'usage_update' }> } | undefined;
  for (const envelope of events) {
    const event = envelope.event;
    const key = stateKey(event) ?? (event.type === 'plan_updated' ? event.type : undefined);
    if (key !== undefined) latest.set(key, envelope);
    if (event.type === 'compaction_status' && event.phase === 'complete') usage = undefined;
    if (event.type !== 'usage_update') continue;
    if (!event.is_snapshot && event.input_tokens === 0 && event.output_tokens === 0
      && event.cache_read_tokens === 0 && event.cache_creation_tokens === 0) continue;
    const outputOnly = !event.is_snapshot && event.input_tokens === 0
      && event.cache_read_tokens === 0 && event.cache_creation_tokens === 0 && event.output_tokens > 0;
    usage = { ...envelope, event: {
      ...event,
      is_snapshot: true,
      ...(outputOnly && usage ? {
        input_tokens: usage.event.input_tokens,
        cache_read_tokens: usage.event.cache_read_tokens,
        cache_creation_tokens: usage.event.cache_creation_tokens,
      } : {}),
    } };
  }
  if (usage) latest.set('usage_update', usage);
  return [...latest.values()].sort((left, right) => left.sequence - right.sequence).map(envelope => envelope.event);
}

function stateKey(event: ClientEvent): string | undefined {
  switch (event.type) {
    case 'status_snapshot':
    case 'session_storage_state':
      return event.type;
    case 'ui_status':
      return `${event.type}:${event.plugin}`;
    default:
      return undefined;
  }
}

/**
 * A source-backed base, an equivalent checkpoint of older stream frames, and
 * a bounded raw tail. Checkpoint entries preserve content and identity events;
 * only adjacent text/reasoning and superseded tool heartbeats are rewritten.
 * Entries are replaced, never mutated, so captured snapshots remain stable.
 */
export class RuntimeEventReplay {
  private base: Envelope | undefined;
  private first: Entry | undefined;
  private last: Entry | undefined;
  private entries = 0;
  private tail: Envelope[] = [];
  private tailStart = 0;
  private readonly states = new Map<string, Entry>();
  private readonly heartbeats = new Map<string, Entry>();
  private delta: Entry | undefined;
  private lastCheckpointSequence = 0;
  private compactedThrough = 0;

  get length(): number { return (this.base ? 1 : 0) + this.entries + this.tail.length; }
  /** A cursor inside rewritten frames must receive a snapshot, not a suffix. */
  get compactedThroughSequence(): number { return this.compactedThrough; }

  reset(base: Envelope): void {
    this.base = base;
    this.first = this.last = this.delta = undefined;
    this.entries = 0;
    this.tail = [];
    this.tailStart = 0;
    this.states.clear();
    this.heartbeats.clear();
    this.lastCheckpointSequence = base.sequence;
    this.compactedThrough = 0;
  }

  append(envelope: Envelope): void {
    const key = stateKey(envelope.event);
    if (key !== undefined) {
      const previous = this.states.get(key);
      if (previous) this.remove(previous);
      // Snapshot-style state may also still be in the small raw tail.
      const tail = this.tailEvents();
      const retained = tail.filter(entry => stateKey(entry.event) !== key);
      if (retained.length !== tail.length) {
        this.tail = retained;
        this.tailStart = 0;
      }
      this.delta = undefined;
    }
    if (this.tail.length < REPLAY_LIVE_TAIL_LIMIT) {
      this.tail.push(envelope);
      return;
    }
    const oldest = this.tail[this.tailStart]!;
    this.tail[this.tailStart] = envelope;
    this.tailStart = (this.tailStart + 1) % REPLAY_LIVE_TAIL_LIMIT;
    this.checkpoint(oldest);
  }

  snapshot(): Envelope[] {
    const out = this.base ? [this.base] : [];
    for (let entry = this.first; entry; entry = entry.next) out.push(entry.envelope);
    out.push(...this.tailEvents());
    return out;
  }

  private tailEvents(): Envelope[] {
    return this.tailStart === 0 ? this.tail.slice()
      : [...this.tail.slice(this.tailStart), ...this.tail.slice(0, this.tailStart)];
  }

  private appendEntry(envelope: Envelope): Entry {
    const entry: Entry = { envelope, previous: this.last };
    if (this.last) this.last.next = entry;
    else this.first = entry;
    this.last = entry;
    this.entries += 1;
    return entry;
  }

  private remove(entry: Entry): void {
    if (entry.previous) entry.previous.next = entry.next;
    else this.first = entry.next;
    if (entry.next) entry.next.previous = entry.previous;
    else this.last = entry.previous;
    const key = stateKey(entry.envelope.event);
    if (key !== undefined) this.states.delete(key);
    if (this.delta === entry) this.delta = undefined;
    this.entries -= 1;
  }

  private checkpoint(envelope: Envelope): void {
    const event = envelope.event;
    const contiguous = envelope.sequence === this.lastCheckpointSequence + 1;
    this.lastCheckpointSequence = envelope.sequence;
    if (!contiguous) this.delta = undefined;
    const previous = this.delta?.envelope;
    let combined: ClientEvent | undefined;
    if (event.type === 'text_delta' && previous?.event.type === 'text_delta') {
      combined = { type: 'text_delta', text: previous.event.text + event.text };
    } else if (event.type === 'thinking_delta' && event.signature === undefined
      && previous?.event.type === 'thinking_delta' && previous.event.signature === undefined) {
      combined = { type: 'thinking_delta', thinking: previous.event.thinking + event.thinking };
    }
    if (combined && previous && this.delta) {
      this.remove(this.delta);
      this.delta = this.appendEntry(Object.freeze({
        ...envelope,
        replayFromSequence: previous.replayFromSequence ?? previous.sequence,
        event: combined,
      }));
      this.compactedThrough = Math.max(this.compactedThrough, envelope.sequence);
      return;
    }
    if (event.type === 'tool_heartbeat') {
      const key = `${event.id}\0${event.tool}`;
      const previousHeartbeat = this.heartbeats.get(key);
      if (previousHeartbeat) {
        this.remove(previousHeartbeat);
        this.compactedThrough = Math.max(this.compactedThrough, envelope.sequence);
      }
      this.heartbeats.set(key, this.appendEntry(envelope));
      // A heartbeat changes no content-block boundary. A delta spanning it
      // covers its sequence too; the checkpoint also retains its latest value.
      return;
    }
    const entry = this.appendEntry(envelope);
    if (event.type === 'text_delta' || event.type === 'thinking_delta') {
      this.delta = entry;
      return;
    }
    this.delta = undefined;
    // Never move heartbeats across a lifecycle, identity or retraction event.
    this.heartbeats.clear();
    const key = stateKey(event);
    if (key !== undefined) this.states.set(key, entry);
  }
}
