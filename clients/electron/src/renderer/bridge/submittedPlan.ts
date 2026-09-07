import type { ClientEvent, MessageDto, PermissionRequest } from '@lingxi/bridge-client';

export interface SubmittedPlan {
  readonly id: string;
  readonly content: string;
  readonly status: 'submitted' | 'pending' | 'approved' | 'rejected' | 'failed';
}
interface Submission extends SubmittedPlan {
  readonly requestId?: number;
  readonly finished?: boolean;
  readonly resultReceived?: boolean;
}
export interface SubmittedPlanState {
  readonly calls: readonly Submission[];
  readonly waitingRequest?: PermissionRequest;
  /** Gate request IDs increase monotonically; a watermark rejects old replay. */
  readonly lastResolvedRequestId?: number;
  readonly resolutions: Readonly<Record<number, 'approved' | 'denied' | 'cancelled' | 'expired'>>;
}
export function emptySubmittedPlanState(): SubmittedPlanState {
  return { calls: [], resolutions: {} };
}
export function latestSubmittedPlan(state: SubmittedPlanState): SubmittedPlan | null {
  const call = [...state.calls].reverse().find((entry) => entry.content.trim().length > 0);
  return call ? { id: call.id, content: call.content, status: call.status } : null;
}
function object(json: string): Record<string, unknown> {
  try {
    const value: unknown = JSON.parse(json);
    return value !== null && typeof value === 'object' && !Array.isArray(value)
      ? value as Record<string, unknown> : {};
  } catch { return {}; }
}
function content(value: Record<string, unknown>): string {
  return typeof value.plan === 'string' && value.plan.trim() ? value.plan : '';
}
function resolutionStatus(resolution: string): SubmittedPlan['status'] {
  return resolution === 'approved' ? 'approved' : resolution === 'denied' ? 'rejected' : 'failed';
}
export function reduceSubmittedPlanPermission(
  state: SubmittedPlanState, request: PermissionRequest, sessionId: string,
): SubmittedPlanState {
  if (request.kind.type !== 'exit_plan_mode' || request.worker
    || request.owner?.worker_name
    || (request.owner?.session_id && request.owner.session_id !== sessionId)) return state;
  if (state.calls.some((call) => call.requestId === request.request_id)) return state;
  if (request.request_id <= (state.lastResolvedRequestId ?? -1)
    && state.waitingRequest?.request_id !== request.request_id) return state;
  // PermissionRequest has no tool-use ID. Root approvals are serialized by the
  // tool gate, so associate with the latest unfinished root submission; keep a
  // request that races its tool-start event until that ID becomes available.
  const target = [...state.calls].reverse().find((call) => !call.finished && call.requestId === undefined);
  if (!target) return { ...state, waitingRequest: request };
  const body = request.kind.plan;
  const resolution = state.resolutions[request.request_id];
  return {
    ...state,
    waitingRequest: undefined,
    resolutions: {},
    calls: state.calls.map((call) => call.id === target.id ? {
      ...call,
      content: body.trim() ? body : call.content,
      requestId: request.request_id,
      status: resolution ? resolutionStatus(resolution) : 'pending',
      finished: resolution !== undefined,
    } : call),
  };
}
function started(state: SubmittedPlanState, id: string, json: string, sessionId: string): SubmittedPlanState {
  // Replayed starts must neither reorder submissions nor downgrade approval data.
  if (state.calls.some((call) => call.id === id)) return state;
  const next: SubmittedPlanState = { ...state, calls: [...state.calls, { id, content: content(object(json)), status: 'submitted' }] };
  return next.waitingRequest ? reduceSubmittedPlanPermission(next, next.waitingRequest, sessionId) : next;
}
function result(state: SubmittedPlanState, id: string, json: string, isError: boolean, historical: boolean): SubmittedPlanState {
  const value = object(json);
  const body = content(value);
  let prior = state.calls.find((call) => call.id === id);
  if (!prior) {
    if (isError || !body) return state;
    prior = { id, content: body, status: 'submitted' };
    state = { ...state, calls: [...state.calls, prior] };
  }
  if (prior.resultReceived && (historical || prior.status !== 'submitted')) return state;
  const status = prior.status === 'rejected' ? 'rejected' : isError ? 'failed'
    : !historical || value.plan_mode === false ? 'approved' : prior.status;
  return { ...state, calls: state.calls.map((call) => call.id === id ? {
    ...call,
    content: !isError && body ? body : call.content,
    status,
    finished: true,
    resultReceived: true,
  } : call) };
}
export function reduceSubmittedPlanMessages(
  state: SubmittedPlanState, messages: readonly MessageDto[], sessionId: string,
): SubmittedPlanState {
  let next = state;
  for (const message of messages) for (const block of message.blocks) {
    if ((block.type === 'tool_use' || block.type === 'tool_result') && block.tool === 'ExitPlanMode') {
      next = block.type === 'tool_use'
        ? started(next, block.id, block.input_json, sessionId)
        : result(next, block.id, block.result_json, block.is_error, true);
    }
  }
  return next;
}
export function reduceSubmittedPlanEvent(
  state: SubmittedPlanState, event: ClientEvent, sessionId: string,
): SubmittedPlanState {
  switch (event.type) {
    case 'tool_use_started':
      return event.tool === 'ExitPlanMode' ? started(state, event.id, event.input_json, sessionId) : state;
    case 'tool_use_result':
      return event.tool === 'ExitPlanMode' ? result(state, event.id, event.result_json, event.is_error, false) : state;
    case 'permission_request_resolved': {
      const waiting = state.waitingRequest?.request_id === event.request_id;
      return {
        ...state,
        lastResolvedRequestId: Math.max(state.lastResolvedRequestId ?? -1, event.request_id),
        // Only a request already waiting for its tool-start needs deferred data.
        // All other resolutions are represented by the constant-size watermark.
        resolutions: waiting ? { [event.request_id]: event.resolution } : state.resolutions,
        calls: state.calls.map((call) => call.requestId === event.request_id
          && !(call.resultReceived && call.status === 'approved') ? {
            ...call, status: resolutionStatus(event.resolution), finished: true,
          } : call),
      };
    }
    case 'session_resumed': {
      if (event.session_id !== sessionId) return state;
      const restored = reduceSubmittedPlanMessages(emptySubmittedPlanState(), event.messages, sessionId);
      // The host can deliver a parked approval before the history snapshot.
      // Preserve only live approvals without a tool result, never stale plans.
      const pending = state.calls.filter((call) => call.requestId !== undefined && !call.resultReceived);
      const calls = restored.calls.map((call) => pending.find((live) => live.id === call.id) ?? call);
      calls.push(...pending.filter((live) => !calls.some((call) => call.id === live.id)));
      const merged: SubmittedPlanState = {
        ...restored, calls,
        waitingRequest: state.waitingRequest,
        resolutions: state.resolutions,
        lastResolvedRequestId: state.lastResolvedRequestId,
      };
      return merged.waitingRequest
        ? reduceSubmittedPlanPermission(merged, merged.waitingRequest, sessionId) : merged;
    }
    case 'message_complete':
      return event.message ? reduceSubmittedPlanMessages(state, [event.message], sessionId) : state;
    default: return state;
  }
}
