/**
 * `useBridge()` — the renderer's live-conversation store (M10 A1 — C3).
 *
 * Subscribes to `window.lingxi.onEvent` / `onConnectionStateChanged`, folds the
 * inbound {@link ClientEvent} stream through the pure {@link reduceEvent} reducer,
 * and exposes the accumulated {@link ConversationState} plus a `sendPrompt` that
 * optimistically echoes the user's message before the engine streams its reply.
 *
 * When `window.lingxi` is absent (the design preview running in a plain browser),
 * the hook reports `hosted === false` and `connected === false`, so callers fall
 * back to the existing mock RUN — the static design preview keeps working.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  ClientEvent,
  PermissionRequest,
  PermissionResponseDto,
} from '@lingxi/bridge-client';
import type { ConnectionState } from './lingxi';
import {
  appendUserPrompt,
  emptyConversation,
  reduceEvent,
  type ConversationState,
  type UsageSnapshot,
} from './conversation';

/** What `useBridge` returns to the renderer. */
export interface UseBridge {
  /** True when running inside the Electron host (i.e. `window.lingxi` exists). */
  readonly hosted: boolean;
  /** The coarse connection lifecycle (always `idle` when not hosted). */
  readonly connection: ConnectionState;
  /** True once the bridge socket is connected. */
  readonly connected: boolean;
  /** The accumulated live conversation (drives the Stage when connected). */
  readonly conversation: ConversationState;
  /** Latest live token-usage snapshot (`usage_update`), or `null`. */
  readonly usage: UsageSnapshot | null;
  /** True while a turn is streaming (drives the composer's thinking affordance). */
  readonly running: boolean;
  /**
   * The oldest still-unanswered {@link PermissionRequest}, or `null`. Drives the
   * allow/deny prompt; cleared once the user responds (or another arrives).
   */
  readonly pendingPermission: PermissionRequest | null;
  /** Submit a prompt: echo it immediately, then drive a turn via the host. */
  sendPrompt(text: string): void;
  /** Cancel the in-flight turn (optionally a specific `turnId`). */
  cancel(turnId?: number): void;
  /** Approve the given permission request (defaults to allow-once). */
  approve(requestId: number, response?: PermissionResponseDto): void;
  /** Deny the given permission request. */
  deny(requestId: number): void;
}

/** Detect the Electron host once (stable across renders). */
function getHost() {
  return typeof window !== 'undefined' ? window.lingxi : undefined;
}

export function useBridge(): UseBridge {
  const hostRef = useRef(getHost());
  const host = hostRef.current;
  const hosted = host !== undefined;

  const [connection, setConnection] = useState<ConnectionState>({ status: 'idle' });
  const [conversation, setConversation] = useState<ConversationState>(emptyConversation);
  // FIFO queue of parked permission requests; the head is rendered as the prompt.
  // Queueing (rather than a single slot) means a second request that arrives
  // before the first is answered is not silently dropped.
  const [permissionQueue, setPermissionQueue] = useState<PermissionRequest[]>([]);

  // Subscribe to the live feed + connection lifecycle for the app's lifetime.
  useEffect(() => {
    if (!host) return;

    const offEvent = host.onEvent((event: ClientEvent) => {
      setConversation((prev) => reduceEvent(prev, event));
    });
    const offState = host.onConnectionStateChanged((state) => {
      setConnection(state);
    });
    const offPermission = host.onPermission((request: PermissionRequest) => {
      // Replace any duplicate of the same request id, else append.
      setPermissionQueue((prev) => [
        ...prev.filter((r) => r.request_id !== request.request_id),
        request,
      ]);
    });

    // Pull the current state once in case we mounted after the first transition.
    void host.connectionState().then(setConnection).catch(() => {
      /* host not ready — the subscription will deliver the next transition. */
    });

    return () => {
      offEvent();
      offState();
      offPermission();
    };
  }, [host]);

  const sendPrompt = useCallback(
    (text: string) => {
      const trimmed = text.trim();
      if (!trimmed) return;
      // Optimistic echo: the user message shows immediately, before the engine
      // streams anything back.
      setConversation((prev) => appendUserPrompt(prev, trimmed));
      if (host) {
        void host.sendPrompt(trimmed).catch(() => {
          setConversation((prev) =>
            reduceEvent(prev, {
              type: 'error',
              kind: { type: 'transport' },
              message: 'failed to send prompt to the engine',
            }),
          );
        });
      }
    },
    [host],
  );

  const cancel = useCallback(
    (turnId?: number) => {
      if (host) void host.cancel(turnId).catch(() => undefined);
    },
    [host],
  );

  /** Drop the head of the queue (the just-answered request) regardless of outcome. */
  const dropPending = useCallback((requestId: number) => {
    setPermissionQueue((prev) => prev.filter((r) => r.request_id !== requestId));
  }, []);

  const approve = useCallback(
    (requestId: number, response?: PermissionResponseDto) => {
      dropPending(requestId);
      if (host) void host.approve(requestId, response).catch(() => undefined);
    },
    [host, dropPending],
  );

  const deny = useCallback(
    (requestId: number) => {
      dropPending(requestId);
      if (host) void host.deny(requestId).catch(() => undefined);
    },
    [host, dropPending],
  );

  const connected = connection.status === 'connected';

  return {
    hosted,
    connection,
    connected,
    conversation,
    usage: conversation.usage,
    running: conversation.running,
    pendingPermission: permissionQueue[0] ?? null,
    sendPrompt,
    cancel,
    approve,
    deny,
  };
}
