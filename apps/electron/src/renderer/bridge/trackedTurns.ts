import type { ClientEvent } from '@lingxi/bridge-client';
import type { DesktopTurnToken, TrackedPromptPurpose, TrackedSpeechEvent } from './bridgeTypes.js';

export const MAX_TRACKED_SPEECH_SUBSCRIBERS = 64;

export interface ActiveTrackedTurn {
  token: DesktopTurnToken;
  text: string;
  sequence: number;
  turnId?: number;
  completed: boolean;
}

export type TrackedSpeechListener = (event: TrackedSpeechEvent) => void;

export function createDesktopTurnToken(
  sessionId: string,
  sequence: number,
  purpose: TrackedPromptPurpose,
): DesktopTurnToken {
  return { sessionId, clientTurnId: `${sessionId}:tracked:${sequence}`, purpose };
}

export function enqueueTrackedTurn(
  pending: Map<string, DesktopTurnToken[]>,
  token: DesktopTurnToken,
): void {
  const queue = pending.get(token.sessionId);
  if (queue) queue.push(token);
  else pending.set(token.sessionId, [token]);
}

export function dequeueTrackedTurn(
  pending: Map<string, DesktopTurnToken[]>,
  token: DesktopTurnToken,
): void {
  const queue = pending.get(token.sessionId);
  if (!queue) return;
  const next = queue.filter((entry) => entry.clientTurnId !== token.clientTurnId);
  if (next.length === 0) pending.delete(token.sessionId);
  else pending.set(token.sessionId, next);
}

function eventClientTurnId(event: Extract<ClientEvent, { type: 'turn_started' }>): string | undefined {
  const clientTurnId = (event as { client_turn_id?: unknown }).client_turn_id;
  return typeof clientTurnId === 'string' && clientTurnId.length > 0 ? clientTurnId : undefined;
}

export function trackedListenerKey(token: DesktopTurnToken): string {
  return token.clientTurnId;
}

export function emitTrackedSpeech(
  listeners: Map<string, Set<TrackedSpeechListener>>,
  event: TrackedSpeechEvent,
): void {
  const subscribers = listeners.get(trackedListenerKey(event.token));
  if (!subscribers || subscribers.size === 0) return;
  for (const listener of [...subscribers]) listener(event);
}

export function bindTrackedTurn(
  pending: Map<string, DesktopTurnToken[]>,
  active: Map<string, ActiveTrackedTurn>,
  sessionId: string,
  event: Extract<ClientEvent, { type: 'turn_started' }>,
): ActiveTrackedTurn | null {
  const queue = pending.get(sessionId);
  if (!queue || queue.length === 0) return null;
  const explicitClientTurnId = eventClientTurnId(event);
  const index = explicitClientTurnId
    ? Math.max(0, queue.findIndex((entry) => entry.clientTurnId === explicitClientTurnId))
    : 0;
  const [token] = queue.splice(index, 1);
  if (!token) return null;
  if (queue.length === 0) pending.delete(sessionId);
  const tracked = { token, text: '', sequence: 0, turnId: event.turn_id, completed: false };
  active.set(sessionId, tracked);
  return tracked;
}

export function appendTrackedTurnDelta(
  active: Map<string, ActiveTrackedTurn>,
  sessionId: string,
  text: string,
): ActiveTrackedTurn | null {
  if (!text) return null;
  const tracked = active.get(sessionId);
  if (!tracked || tracked.completed) return null;
  tracked.text += text;
  tracked.sequence += 1;
  return tracked;
}

export function completeTrackedTurn(
  active: Map<string, ActiveTrackedTurn>,
  sessionId: string,
): ActiveTrackedTurn | null {
  const tracked = active.get(sessionId);
  if (!tracked || tracked.completed) return null;
  tracked.completed = true;
  active.delete(sessionId);
  return tracked;
}

export function clearTrackedTurnState(
  pending: Map<string, DesktopTurnToken[]>,
  active: Map<string, ActiveTrackedTurn>,
  listeners: Map<string, Set<TrackedSpeechListener>>,
  sessionId: string,
): DesktopTurnToken[] {
  const cleared: DesktopTurnToken[] = pending.get(sessionId) ?? [];
  pending.delete(sessionId);
  const tracked = active.get(sessionId);
  if (tracked) {
    active.delete(sessionId);
    cleared.push(tracked.token);
  }
  for (const token of cleared) listeners.delete(trackedListenerKey(token));
  return cleared;
}
