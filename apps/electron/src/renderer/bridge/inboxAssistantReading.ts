import { freezeRead, type InboxItem, type InboxMutation } from '../../../../../packages/inbox/contracts';

export interface AssistantInboxActivity {
  sourceId: string;
  cursor: number;
  baselineCursor: number;
  latestResultUuids: string[];
}

/** Catalog activity is source-owned metadata, never a transcript row count. */
export function assistantInboxActivity(json: string | null | undefined): AssistantInboxActivity | null {
  if (!json || json.length > 16_384) return null;
  try {
    const value: unknown = JSON.parse(json);
    if (!value || typeof value !== 'object') return null;
    const data = value as Record<string, unknown>;
    if (typeof data.sourceId !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(data.sourceId)
      || !Number.isSafeInteger(data.cursor) || (data.cursor as number) < 0
      || !Number.isSafeInteger(data.baselineCursor) || (data.baselineCursor as number) < 0
      || (data.baselineCursor as number) > (data.cursor as number)) return null;
    const ids = Array.isArray(data.latestResultUuids) ? data.latestResultUuids : [];
    return {
      sourceId: data.sourceId, cursor: data.cursor as number, baselineCursor: data.baselineCursor as number,
      latestResultUuids: ids.filter((id): id is string => typeof id === 'string'
        && /^[0-9a-f-]{36}$/i.test(id)).slice(0, 16),
    };
  } catch { return null; }
}

export interface AssistantReadingPosition {
  sessionKey: string;
  visibleMessageUuids: readonly string[];
  /** Search and notification targets are previews until the user returns to the timeline. */
  preview: boolean;
  explicitLatest?: boolean;
  latestIntent?: number;
}

export interface AssistantReadingEntry { cursor: number; resultUuids: readonly string[] }

interface Observation {
  item: InboxItem;
  activity: AssistantInboxActivity;
  position: AssistantReadingPosition;
  foreground: boolean;
  entries?: readonly AssistantReadingEntry[];
}

interface ReadingClock {
  now(): number;
  schedule(callback: () => void, delay: number): ReturnType<typeof setTimeout>;
  cancel(timer: ReturnType<typeof setTimeout>): void;
}
const realClock: ReadingClock = {
  now: Date.now,
  schedule: (callback, delay) => setTimeout(callback, delay),
  cancel: timer => clearTimeout(timer),
};

/** Only a rendered, committed result can acknowledge its semantic activity cursor. */
export class AssistantInboxReader {
  private owner = '';
  private entryManualRevision = 0;
  private exposures = new Map<number, ReturnType<typeof setTimeout>>();
  private confirmed = new Set<number>();
  private frontier = 0;
  private jumpThrough?: number;
  private lastJumpIntent?: number;
  private lastJumpOwner = '';
  private submitTimer?: ReturnType<typeof setTimeout>;
  private pending?: Extract<InboxMutation, { action: 'markRead' }>;
  private lastSubmittedAt = -Infinity;
  private lastAdmitted = '';

  constructor(private readonly submit: (command: InboxMutation) => Promise<unknown>,
    private readonly failed: (error: unknown) => void,
    private readonly clock: ReadingClock = realClock) {}

  observe({ item, activity, position, foreground, entries }: Observation): void {
    const owner = `${item.key}:${item.ref.generation}`;
    if (this.owner !== owner) {
      this.leave();
      this.owner = owner;
      this.entryManualRevision = item.manualUnreadRevision;
      this.lastAdmitted = '';
      this.frontier = Math.max(item.readThrough, activity.baselineCursor);
    }
    this.frontier = Math.max(this.frontier, item.readThrough, activity.baselineCursor);
    const intent = position.latestIntent ?? 0;
    if (position.explicitLatest && (owner !== this.lastJumpOwner || intent !== this.lastJumpIntent)) {
      this.lastJumpOwner = owner;
      this.lastJumpIntent = intent;
      this.jumpThrough = item.sourceCursor;
      this.confirmed.delete(item.sourceCursor);
      const timer = this.exposures.get(item.sourceCursor);
      if (timer !== undefined) this.clock.cancel(timer);
      this.exposures.delete(item.sourceCursor);
    }
    const explicitLatest = position.explicitLatest && this.jumpThrough === item.sourceCursor;
    const exposed = foreground && (!position.preview || position.explicitLatest) && item.ref.kind === 'assistant'
      && item.ref.conversationId === position.sessionKey && item.ref.sourceId === activity.sourceId
      && item.sourceCursor <= activity.cursor;
    if (!exposed) { this.cancelExposure(); return; }
    const candidates = entries ?? [{ cursor: activity.cursor, resultUuids: activity.latestResultUuids }];
    const visible = candidates.filter(entry => entry.cursor <= item.sourceCursor
      && entry.resultUuids.some(id => position.visibleMessageUuids.includes(id)));
    // Explicit latest may confirm preceding results, but still requires the actual latest result on screen.
    if (explicitLatest && activity.cursor === item.sourceCursor
      && activity.latestResultUuids.some(id => position.visibleMessageUuids.includes(id))) {
      if (!visible.some(entry => entry.cursor === item.sourceCursor)) visible.push({ cursor: item.sourceCursor, resultUuids: activity.latestResultUuids });
    }
    const visibleCursors = new Set(visible.map(entry => entry.cursor));
    for (const [cursor, timer] of this.exposures) {
      if (!visibleCursors.has(cursor)) { this.clock.cancel(timer); this.exposures.delete(cursor); }
    }
    for (const entry of visible) {
      if (this.exposures.has(entry.cursor) || this.confirmed.has(entry.cursor)) continue;
      this.exposures.set(entry.cursor, this.clock.schedule(() => {
        this.exposures.delete(entry.cursor);
        this.confirmed.add(entry.cursor);
        if (explicitLatest && entry.cursor === item.sourceCursor) { this.frontier = item.sourceCursor; this.jumpThrough = undefined; }
        while (this.confirmed.delete(this.frontier + 1)) this.frontier++;
        if (this.frontier <= item.readThrough && !item.manualUnread) return;
        const command = { action: 'markRead' as const, ...freezeRead(item, this.frontier), manualUnreadRevision: this.entryManualRevision };
        const identity = `${owner}:${command.through}:${this.entryManualRevision}`;
        if (identity === this.lastAdmitted) return;
        this.pending = command;
        const remaining = 1000 - (this.clock.now() - this.lastSubmittedAt);
        if (remaining <= 0) this.flush();
        else {
          if (this.submitTimer !== undefined) this.clock.cancel(this.submitTimer);
          this.submitTimer = this.clock.schedule(() => this.flush(), remaining);
        }
      }, 300));
    }
  }

  private cancelExposure(): void {
    for (const timer of this.exposures.values()) this.clock.cancel(timer);
    this.exposures.clear();
  }

  /** Flush only positions that already passed the exposure gate. */
  flush(): void {
    if (this.submitTimer !== undefined) this.clock.cancel(this.submitTimer);
    this.submitTimer = undefined;
    const command = this.pending;
    if (!command) return;
    this.pending = undefined;
    this.lastSubmittedAt = this.clock.now();
    this.lastAdmitted = `${command.key}:${command.generation}:${command.through}:${command.manualUnreadRevision}`;
    void this.submit(command).catch(error => { this.lastAdmitted = ''; this.failed(error); });
  }

  leave(): void {
    this.cancelExposure();
    this.flush();
    this.owner = '';
    this.confirmed.clear();
    this.jumpThrough = undefined;
  }
}
