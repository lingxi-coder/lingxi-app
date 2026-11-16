export interface SequencedRuntimeEventEnvelope<T = unknown> {
  sessionId: string;
  sequence: number;
  /** The cached event replaces all stream frames in this inclusive span. */
  replayFromSequence?: number;
  event: T;
}

/** Merge cached and concurrently-buffered runtime events exactly once. */
export function mergeRuntimeEventReplay<T>(
  replay: readonly SequencedRuntimeEventEnvelope<T>[],
  live: readonly SequencedRuntimeEventEnvelope<T>[],
): SequencedRuntimeEventEnvelope<T>[] {
  const unique = new Map<string, SequencedRuntimeEventEnvelope<T>>();
  const covered = new Map<string, Array<readonly [number, number]>>();
  const valid = (envelope: SequencedRuntimeEventEnvelope<T>): boolean =>
    Number.isSafeInteger(envelope.sequence) && envelope.sequence >= 1 && Boolean(envelope.sessionId);
  for (const envelope of replay) {
    if (!valid(envelope)) continue;
    const from = envelope.replayFromSequence;
    if (from !== undefined && Number.isSafeInteger(from) && from >= 1 && from <= envelope.sequence) {
      const ranges = covered.get(envelope.sessionId) ?? [];
      ranges.push([from, envelope.sequence]);
      covered.set(envelope.sessionId, ranges);
    }
    unique.set(`${envelope.sessionId}\0${envelope.sequence}`, envelope);
  }
  for (const [sessionId, ranges] of covered) {
    ranges.sort((left, right) => left[0] - right[0]);
    const merged: Array<readonly [number, number]> = [];
    for (const range of ranges) {
      const previous = merged.at(-1);
      if (previous && range[0] <= previous[1] + 1) merged[merged.length - 1] = [previous[0], Math.max(previous[1], range[1])];
      else merged.push(range);
    }
    covered.set(sessionId, merged);
  }
  const isCovered = (sessionId: string, sequence: number): boolean => {
    const ranges = covered.get(sessionId);
    if (!ranges) return false;
    let low = 0;
    let high = ranges.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      const [from, through] = ranges[middle]!;
      if (sequence < from) high = middle;
      else if (sequence > through) low = middle + 1;
      else return true;
    }
    return false;
  };
  for (const envelope of live) {
    if (!valid(envelope)) continue;
    // The replay span is authoritative even at its final sequence: the live
    // frame there carries only the last fragment of its accumulated text.
    if (isCovered(envelope.sessionId, envelope.sequence)) continue;
    unique.set(`${envelope.sessionId}\0${envelope.sequence}`, envelope);
  }
  return [...unique.values()].sort((left, right) => (
    left.sessionId.localeCompare(right.sessionId) || left.sequence - right.sequence
  ));
}

export function createRuntimeEventReplayBuffer<T>(
  deliver: (envelope: SequencedRuntimeEventEnvelope<T>) => void,
  reportError: (error: unknown) => void = error => console.error('Runtime event delivery failed', error),
) {
  let active = true;
  let replayPending = true;
  const buffered: SequencedRuntimeEventEnvelope<T>[] = [];
  const emit = (envelope: SequencedRuntimeEventEnvelope<T>): void => {
    // A bad background event must not discard the rest of a cold window's
    // replay, including another session's saved transcript.
    try { deliver(envelope); } catch (error) { reportError(error); }
  };
  return {
    push(envelope: SequencedRuntimeEventEnvelope<T>): void {
      if (!active) return;
      if (replayPending) buffered.push(envelope);
      else emit(envelope);
    },
    resolve(replay: readonly SequencedRuntimeEventEnvelope<T>[]): void {
      if (!active || !replayPending) return;
      replayPending = false;
      for (const envelope of mergeRuntimeEventReplay(replay, buffered.splice(0))) emit(envelope);
    },
    reject(): void {
      this.resolve([]);
    },
    dispose(): void {
      active = false;
      buffered.length = 0;
    },
  };
}

export interface RuntimeReplayDiagnostic {
  phase: 'preload_request' | 'preload_retry' | 'preload_received' | 'preload_failed';
  attempt: number;
  durationMs: number;
  eventCount?: number;
}

/** Keep live events buffered while a transient cold-start IPC failure recovers. */
export function requestRuntimeEventReplay<T>(
  request: () => Promise<readonly SequencedRuntimeEventEnvelope<T>[]>,
  buffer: Pick<ReturnType<typeof createRuntimeEventReplayBuffer<T>>, 'resolve' | 'reject'>,
  reportError: (error: unknown) => void = error => console.error('Runtime event replay failed', error),
  observe?: (sample: RuntimeReplayDiagnostic) => void,
): () => void {
  let active = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let expire: (() => void) | undefined;
  const attempt = async (remaining: number): Promise<void> => {
    const started = Date.now();
    const trace = (phase: RuntimeReplayDiagnostic['phase'], eventCount?: number): void => {
      try {
        observe?.({ phase, attempt: 3 - remaining, durationMs: Math.max(0, Date.now() - started),
          ...(eventCount !== undefined ? { eventCount } : {}) });
      } catch { /* Diagnostics never own recovery. */ }
    };
    trace('preload_request');
    let deadline: ReturnType<typeof setTimeout> | undefined;
    try {
      const timeout = new Promise<never>((_resolve, reject) => {
        expire = () => reject(new Error('Runtime event replay timed out'));
        deadline = setTimeout(expire, 2000);
      });
      const replay = await Promise.race([request(), timeout]);
      if (active) {
        trace('preload_received', replay.length);
        buffer.resolve(replay);
      }
    } catch (error) {
      if (!active) return;
      if (remaining > 0) {
        trace('preload_retry');
        timer = setTimeout(() => { timer = undefined; void attempt(remaining - 1); }, 250);
      } else {
        trace('preload_failed');
        reportError(error);
        buffer.reject();
      }
    } finally {
      if (deadline !== undefined) clearTimeout(deadline);
      expire = undefined;
    }
  };
  void attempt(2);
  return () => { active = false; if (timer !== undefined) clearTimeout(timer); expire?.(); };
}
