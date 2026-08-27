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

export function createRuntimeEventReplayBuffer<T>(deliver: (envelope: SequencedRuntimeEventEnvelope<T>) => void) {
  let active = true;
  let replayPending = true;
  const buffered: SequencedRuntimeEventEnvelope<T>[] = [];
  return {
    push(envelope: SequencedRuntimeEventEnvelope<T>): void {
      if (!active) return;
      if (replayPending) buffered.push(envelope);
      else deliver(envelope);
    },
    resolve(replay: readonly SequencedRuntimeEventEnvelope<T>[]): void {
      if (!active || !replayPending) return;
      replayPending = false;
      for (const envelope of mergeRuntimeEventReplay(replay, buffered.splice(0))) deliver(envelope);
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
