import type { CronJobDto } from '@lingxi/bridge-client';

const CLAIM_PREFIX = 'lingxi-cron-claim-v1:';
const CANCEL_REQUESTED_MARKER = 'lingxi-host-cancel-requested-v1';

/** Wire correlation belongs to one claim; history and notifications belong to its occurrence. */
export function scheduledRunIdentity(wireId: string, task: CronJobDto): {
  occurrenceId: string;
  claimGeneration: number | null;
  hostAdmitted: boolean;
} {
  const runs = task.automation?.runs ?? [];
  const legacy = {
    occurrenceId: wireId,
    claimGeneration: runs.find((run) => run.id === wireId)?.claimGeneration ?? null,
    hostAdmitted: false,
  };
  if (!wireId.startsWith(CLAIM_PREFIX)) return legacy;
  try {
    const identity: unknown = JSON.parse(wireId.slice(CLAIM_PREFIX.length));
    if (!Array.isArray(identity) || identity.length !== 2
      || typeof identity[0] !== 'string' || identity[0].length === 0
      || !Number.isSafeInteger(identity[1]) || identity[1] < 0) return legacy;
    const [occurrenceId, claimGeneration] = identity as [string, number];
    // Decode only product claims authenticated by their existing SDK metadata.
    // An arbitrary legacy identifier that resembles JSON remains opaque.
    const run = runs.find((entry) => entry.id === occurrenceId && entry.claimGeneration === claimGeneration);
    if (!run) return legacy;
    return {
      occurrenceId, claimGeneration,
      hostAdmitted: run.startedAt != null || run.sessionId != null || run.error === CANCEL_REQUESTED_MARKER,
    };
  } catch {
    return legacy;
  }
}
