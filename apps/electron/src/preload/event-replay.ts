export interface SequencedRuntimeEventEnvelope<T = unknown> {
  sessionId: string;
  sequence: number;
  event: T;
}

/** Merge cached and concurrently-buffered runtime events exactly once. */
export function mergeRuntimeEventReplay<T>(
  replay: readonly SequencedRuntimeEventEnvelope<T>[],
  live: readonly SequencedRuntimeEventEnvelope<T>[],
): SequencedRuntimeEventEnvelope<T>[] {
  const unique = new Map<string, SequencedRuntimeEventEnvelope<T>>();
  for (const envelope of [...replay, ...live]) {
    if (!Number.isSafeInteger(envelope.sequence) || envelope.sequence < 1 || !envelope.sessionId) continue;
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

/** Keep live events buffered while a transient cold-start IPC failure recovers. */
export function requestRuntimeEventReplay<T>(
  request: () => Promise<readonly SequencedRuntimeEventEnvelope<T>[]>,
  buffer: Pick<ReturnType<typeof createRuntimeEventReplayBuffer<T>>, 'resolve' | 'reject'>,
  reportError: (error: unknown) => void = error => console.error('Runtime event replay failed', error),
): () => void {
  let active = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let expire: (() => void) | undefined;
  const attempt = async (remaining: number): Promise<void> => {
    let deadline: ReturnType<typeof setTimeout> | undefined;
    try {
      const timeout = new Promise<never>((_resolve, reject) => {
        expire = () => reject(new Error('Runtime event replay timed out'));
        deadline = setTimeout(expire, 2000);
      });
      const replay = await Promise.race([request(), timeout]);
      if (active) buffer.resolve(replay);
    } catch (error) {
      if (!active) return;
      if (remaining > 0) {
        timer = setTimeout(() => { timer = undefined; void attempt(remaining - 1); }, 250);
      } else {
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
