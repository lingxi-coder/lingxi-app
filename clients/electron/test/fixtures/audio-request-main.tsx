/**
 * Fixture for the engine-audio round trip, mounted in a REAL Electron
 * renderer by `audio-request-electron.mjs`.
 *
 * Why this exists rather than another unit test: `useBridge` subscribes to
 * engine events inside a `useEffect`, and the only renderer available to the
 * `.test.ts` suite is `react-dom/server`'s `renderToString`, which runs a
 * component body and deliberately skips effects. So no unit test can observe
 * the subscription actually being made. The source-text guards in
 * `audio-requests.test.ts` are a poor substitute — proved poor in this very
 * task, where an `includes()` check stayed green through an `if (false && …)`
 * mutation. This fixture runs the real hook in a real browser, where the
 * effect genuinely runs.
 *
 * What is REAL here: `useBridge` itself, its `useEffect`, its subscription to
 * `window.lingxi.onEvent`, its `audio_request` branch, the lazily built
 * `MicrophoneCapture`/`speechSynthesis` bindings, and all of
 * `renderer/audio/requests.ts`.
 *
 * What is a stand-in: `window.lingxi`. In production that object is installed
 * by the preload script over `contextBridge` and its `command` reaches the
 * main process through IPC. Here it is a plain in-page object, so this
 * fixture proves the RENDERER half of the round trip; the main-process half
 * (`validateClientCommand` accepting what the renderer produces) is covered
 * by the gate-seam test in `audio-requests.test.ts`, which runs every
 * producible result through the real validator.
 *
 * The ops driven are `is_recording` and `transcribe` — the two that need no
 * OS device access, so this stays deterministic and never raises a
 * microphone permission prompt in CI.
 */
import { StrictMode, useEffect } from 'react';
import { createRoot } from 'react-dom/client';

import { useBridge } from '../../src/renderer/bridge/useBridge';

type Listener = (envelope: unknown) => void;

const eventListeners = new Set<Listener>();
const commands: { sessionId: string; command: unknown }[] = [];
/** `request_id`s the fake engine refuses, standing in for "this id is unknown to me now". */
const rejectedRequestIds = new Set<number>();

const SESSION_ID = '99999999-8888-4777-8666-555555555555';

function subscribe(set: Set<Listener>, listener: Listener) {
  set.add(listener);
  return () => { set.delete(listener); };
}

// Installed BEFORE the first render: `useBridge` captures the host with
// `useRef(getHost())`, so an object attached later would never be seen.
(window as unknown as { lingxi: unknown }).lingxi = {
  platform: 'darwin',
  isElectron: true,
  onEvent: (callback: Listener) => subscribe(eventListeners, callback),
  onConnectionStateChanged: () => () => {},
  onPermission: () => () => {},
  onComputerAccess: () => () => {},
  onAskUserQuestion: () => () => {},
  onDiagnostic: () => () => {},
  bootstrap: async () => ({
    settings: {
      version: 1,
      theme: 'system',
      projects: [],
      pinnedSessions: [],
      voice: {
        schemaVersion: 2,
        recognitionMode: 'automatic',
        language: 'auto',
        voiceSelection: 'system:default',
        rate: 1,
      },
    },
    workspace: { trusted: false },
    runtimes: [],
    projectCatalogs: {},
    connection: { status: 'idle' },
    diagnostics: [],
  }),
  command: async (sessionId: string, command: unknown) => {
    commands.push({ sessionId, command });
    const requestId = (command as { request_id?: number }).request_id;
    if (typeof requestId === 'number' && rejectedRequestIds.has(requestId)) {
      // Exactly what the engine does with a response it no longer expects.
      throw new Error(`unknown audio request id ${requestId}`);
    }
  },
};

function Fixture() {
  // The real hook. Nothing here reimplements its subscription.
  const bridge = useBridge();

  useEffect(() => {
    window.__audioRequestTest = {
      listenerCount: () => eventListeners.size,
      rejectRequestId: (requestId: number) => { rejectedRequestIds.add(requestId); },
      emit: (requestId: number, op: unknown) => {
        const envelope = {
          sessionId: SESSION_ID,
          sequence: requestId,
          event: { type: 'audio_request', request_id: requestId, op },
        };
        for (const listener of [...eventListeners]) listener(envelope);
      },
      commands: () => commands.map((entry) => ({ ...entry })),
      crashed: () => window.__audioRequestCrashed === true,
    };
    return () => { delete window.__audioRequestTest; };
  }, []);

  return <div id="ready" data-hosted={String(bridge.hosted)}>audio fixture</div>;
}

// Any error escaping the event listener would surface here; the "inert, not
// fatal" claim is only meaningful if something is watching for it.
window.addEventListener('error', () => { window.__audioRequestCrashed = true; });
window.addEventListener('unhandledrejection', () => { window.__audioRequestCrashed = true; });

declare global {
  interface Window {
    __audioRequestCrashed?: boolean;
    __audioRequestTest?: {
      listenerCount(): number;
      rejectRequestId(requestId: number): void;
      emit(requestId: number, op: unknown): void;
      commands(): { sessionId: string; command: unknown }[];
      crashed(): boolean;
    };
  }
}

createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
