import type { ConnectionFailure, FailureKind } from './viewerConnection';
import type { StringKey } from './strings';

const DENIED_DETAIL: Record<string, StringKey> = {
  rate_limited: 'failDeniedRateLimited',
  busy: 'failDeniedBusy',
  timeout: 'failDeniedTimeout',
};

/** One string per kind of failure, so that `Record<FailureKind, …>` fails to compile when a kind is added. */
const KIND_STRINGS: Record<FailureKind, StringKey> = {
  unsupported: 'failUnsupported',
  unknown_host: 'failUnknownHost',
  network: 'failNetwork',
  no_answer: 'failNoAnswer',
  closed: 'failClosed',
  capacity: 'failCapacity',
  busy: 'failBusy',
  revoked: 'failRevoked',
  disabled: 'failDisabled',
  upgrade_required: 'failUpgrade',
  denied: 'failDenied',
  expired: 'failExpired',
  rejected: 'failRejected',
  verification: 'failVerification',
  pairing_code_mismatch: 'failPairingMismatch',
  protocol: 'failProtocol',
};

/** The string that explains a failure to the user. */
export function failureStringKey(failure: ConnectionFailure): StringKey {
  if (failure.kind === 'network' && (failure.detail === '401' || failure.detail === '403')) return 'failNetworkDenied';
  if (failure.kind === 'denied' && failure.detail !== undefined && DENIED_DETAIL[failure.detail] !== undefined) return DENIED_DETAIL[failure.detail]!;
  if (failure.kind === 'revoked' || failure.kind === 'disabled') return KIND_STRINGS[failure.kind];
  return KIND_STRINGS[failure.kind];
}
