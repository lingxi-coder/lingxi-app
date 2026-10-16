import type { CronJobDto } from '@lingxi/bridge-client';

const CLAIM_PREFIX = 'lingxi-cron-claim-v1:';
const CANCEL_REQUESTED_MARKER = 'lingxi-host-cancel-requested-v1';

/** Wire correlation belongs to one claim; history and notifications belong to its occurrence. */
export function scheduledRunIdentity(wireId: string, task: CronJobDto): {
  occurrenceId: string;
  claimGeneration: number;
  hostAdmitted: boolean;
} {
  if (!wireId.startsWith(CLAIM_PREFIX)) {
    throw new Error('invalid scheduled claim identity');
  }
  let identity: unknown;
  try {
    identity = JSON.parse(wireId.slice(CLAIM_PREFIX.length));
  } catch {
    throw new Error('invalid scheduled claim identity');
  }
  if (!Array.isArray(identity) || identity.length !== 2
    || typeof identity[0] !== 'string' || identity[0].length === 0
    || !Number.isSafeInteger(identity[1]) || identity[1] < 0) {
    throw new Error('invalid scheduled claim identity');
  }
  const [occurrenceId, claimGeneration] = identity as [string, number];
  const run = task.automation?.runs?.find((entry) => entry.id === occurrenceId && entry.claimGeneration === claimGeneration);
  if (!run) throw new Error('scheduled claim metadata does not match its wire identity');
  return {
    occurrenceId, claimGeneration,
    hostAdmitted: run.startedAt != null || run.sessionId != null || run.error === CANCEL_REQUESTED_MARKER,
  };
}
