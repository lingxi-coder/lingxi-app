import type { ClientEvent } from '@lingxi/bridge-client';
import type { DesktopTurnToken, TrackedPromptPurpose, TrackedSpeechEvent } from './bridgeTypes.js';

export const MAX_TRACKED_SPEECH_SUBSCRIBERS = 64;

export interface ActiveTrackedTurn {
  token: DesktopTurnToken;
  text: string;
  sequence: number;
  turnId?: number;
  completed: boolean;
  messageText?: string;
  messageId?: string;
  messages?: Array<{ id?: string; text: string; published: boolean }>;
}

export type TrackedSpeechListener = (event: TrackedSpeechEvent) => void;

export function createDesktopTurnToken(
  sessionId: string,
  sequence: number,
  purpose: TrackedPromptPurpose,
  turnId?: number,
): DesktopTurnToken {
  return { sessionId, clientTurnId: `${sessionId}:tracked:${sequence}`, purpose, ...(turnId === undefined ? {} : { turnId }) };
}

/** Random admission IDs remain distinct across renderer reloads and windows. */
export function allocateTrackedTurnId(): number {
  const words = crypto.getRandomValues(new Uint32Array(2));
  return (words[0]! & 0x1fffff) * 0x100000000 + words[1]! || 1;
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
  const correlated = queue.some((entry) => entry.turnId !== undefined);
  const index = explicitClientTurnId
    ? queue.findIndex((entry) => entry.clientTurnId === explicitClientTurnId)
    : correlated ? queue.findIndex((entry) => entry.turnId !== undefined && entry.turnId === event.turn_id) : 0;
  if (index < 0) return null;
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
  tracked.messageText = (tracked.messageText ?? '') + text;
  tracked.sequence += 1;
  return tracked;
}

export function identifyTrackedMessage(active: Map<string, ActiveTrackedTurn>, sessionId: string, messageId: string): void {
  const tracked = active.get(sessionId);
  if (tracked) tracked.messageId = messageId;
}

/** Boundaries seal one model response, while ownership stays with the whole turn. */
export function sealTrackedMessage(active: Map<string, ActiveTrackedTurn>, sessionId: string): void {
  const tracked = active.get(sessionId);
  if (!tracked) return;
  (tracked.messages ??= []).push({ id: tracked.messageId, text: tracked.messageText ?? '', published: false });
  tracked.messageText = '';
  tracked.messageId = undefined;
}

export function retractTrackedMessage(active: Map<string, ActiveTrackedTurn>, sessionId: string, messageId: string): void {
  const tracked = active.get(sessionId);
  if (!tracked) return;
  tracked.messages = (tracked.messages ?? []).filter((message) => message.id !== messageId);
  if (tracked.messageId === messageId) {
    tracked.messageText = '';
    tracked.messageId = undefined;
  }
  tracked.text = tracked.messages.map((message) => message.text).join('') + (tracked.messageText ?? '');
}

/**
 * A retry retracts immediately after its boundary. A following response/tool
 * event or turn terminal confirms that retained messages can enter playback.
 */
export function acceptTrackedMessages(active: Map<string, ActiveTrackedTurn>, sessionId: string): TrackedSpeechEvent[] {
  const tracked = active.get(sessionId);
  if (!tracked) return [];
  const events: TrackedSpeechEvent[] = [];
  for (const message of tracked.messages ?? []) {
    if (message.published) continue;
    message.published = true;
    if (message.text) events.push({ type: 'message', token: tracked.token, text: message.text,
      sequence: tracked.sequence, turnId: tracked.turnId });
  }
  return events;
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
