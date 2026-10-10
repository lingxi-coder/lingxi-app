import type { AudioOperationIdDto, AudioOwnerDto, ClientEvent } from '@lingxi/bridge-client';
import type { ConnectionState } from './bridgeTypes.js';
import { diagnosticEvent } from './host-utils.js';

export const SESSION_RUNTIME_DISPOSED_REASON = 'session runtime disposed';

export function audioIdentityKey(identity: AudioOperationIdDto): string {
  return `${identity.service_epoch}:${identity.generation}:${identity.id}`;
}

export function audioOwnerKey(owner: AudioOwnerDto): string {
  switch (owner.type) {
    case 'session': return `session:${owner.session_id}`;
    case 'local_app': return `local_app:${owner.app_id}:${owner.runtime_generation}`;
    case 'ui': return `ui:${owner.instance_id}`;
    case 'system': return `system:${owner.instance_id}`;
  }
}

export function urlOrigin(raw: string): string | undefined {
  try {
    const url = new URL(raw);
    return url.protocol === 'file:' ? 'file://' : url.origin;
  } catch {
    return undefined;
  }
}

export function connectionDiagnostic(state: ConnectionState, generation: number): string {
  return diagnosticEvent('connection_state', { generation, state });
}

export function childExitDiagnostic(code: number | null, signal: NodeJS.Signals | null, generation: number): string {
  return diagnosticEvent('child_exit', {
    clean: signal === null && code === 0,
    code,
    generation,
    signal,
  });
}

export function bridgeVersionDiagnostic(server: string, serverProtocol: string, clientProtocol: string): string {
  return diagnosticEvent('bridge_handshake', {
    clientProtocol,
    server,
    serverProtocol,
  });
}

export function isTurnOwnedEvent(event: ClientEvent): boolean {
  return [
    'ask_user_question',
    'thinking_delta',
    'tool_use_started',
    'tool_heartbeat',
    'tool_use_result',
    'message_complete',
    'cost_update',
    'usage_update',
    'api_retry',
    'query_model_change',
    'assistant_block_start',
    'assistant_block_identity',
    'tombstone',
    'refusal_continuation',
    'user_transcript_row_identity',
    'assistant_transcript_row_uuids',
  ].includes(event.type);
}

export const TRANSCRIPT_REPLAY_BASE_EVENTS = new Set<ClientEvent['type']>([
  'session_started',
  'session_resumed',
]);

export const TRANSCRIPT_REPLAY_EVENTS = new Set<ClientEvent['type']>([
  'turn_started',
  'turn_ended',
  'text_delta',
  'thinking_delta',
  'tool_use_started',
  'tool_heartbeat',
  'tool_use_result',
  'plan_updated',
  'message_complete',
  'usage_update',
  'status_snapshot',
  'compaction_status',
  'error',
  'message_identity',
  'message_retracted',
  'query_model_change',
  'assistant_block_start',
  'assistant_block_identity',
  'tombstone',
  'refusal_continuation',
  'user_transcript_row_identity',
  'assistant_transcript_row_uuids',
  'system_notice',
  'ui_log',
  'ui_status',
  'loop_wakeup',
  'scheduled_task_fire',
  'session_ended',
]);
