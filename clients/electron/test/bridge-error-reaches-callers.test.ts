import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToString } from 'react-dom/server';

import { useBridge, type UseBridge } from '../src/renderer/bridge/useBridge';
import { defaultVoicePreferences } from '../src/shared/voicePreferences';

/**
 * Whether a failing host IPC call reaches the page that asked for it.
 *
 * Settings pages render inline errors by hanging a `.catch()` off a
 * `bridge.*` method — for example `Plugins`' `saveError`, `Projects`'
 * `error`, and `RawJson`'s `saveError`. All
 * of them depend on one thing being true of `useBridge`'s private `capture`
 * helper: it sets the global `error` AND RETHROWS. If it ever stopped
 * rethrowing, every one of those rows would go silent at once — the write
 * would fail, the inline error would never render, and the only surviving
 * signal would be the global `<ErrorBanner>`, which the Settings shell
 * renders over. Nothing in the type system says `capture` rethrows, so this
 * file says it, by running the real hook.
 *
 * `renderToString` is enough to obtain the hook's return value: it runs the
 * component body (`useState`/`useRef`/`useCallback` all work) and skips
 * effects, which is exactly what is wanted — no sockets, no bootstrap, no
 * DOM.
 */
function bridgeWithHost(host: Record<string, unknown>): UseBridge {
  (globalThis as unknown as { window?: unknown }).window = { lingxi: {
    onConnectionState: () => () => {},
    onEvent: () => () => {},
    onEventReplay: () => () => {},
    onPermission: () => () => {},
    onComputerAccess: () => () => {},
    onAskUserQuestion: () => () => {},
    onDiagnostic: () => () => {},
    bootstrap: async () => { throw new Error('the probe never boots a real session'); },
    ...host,
  } };
  let captured: UseBridge | null = null;
  function Probe() {
    captured = useBridge();
    return null;
  }
  renderToString(createElement(Probe));
  delete (globalThis as unknown as { window?: unknown }).window;
  assert.ok(captured, 'the probe component must have rendered');
  return captured as unknown as UseBridge;
}

test('adding a project refreshes saved state even when initial session startup fails', async () => {
  let refreshed = false;
  const bridge = bridgeWithHost({
    pickWorkspace: async () => { throw new Error('engine startup failed'); },
    bootstrap: async () => {
      refreshed = true;
      return { revision: 1, settings: { projects: ['/saved/project'] }, runtimes: [] };
    },
  });
  await assert.rejects(() => bridge.addProject(), /engine startup failed/);
  assert.equal(refreshed, true);
});

test('project recovery preserves the startup error if refreshing also fails', async () => {
  const bridge = bridgeWithHost({
    pickWorkspace: async () => { throw new Error('engine startup failed'); },
    bootstrap: async () => { throw new Error('refresh failed'); },
  });
  await assert.rejects(() => bridge.addProject(), /engine startup failed/);
});

test('cancelling the project picker does not refresh state', async () => {
  let refreshed = false;
  const bridge = bridgeWithHost({
    pickWorkspace: async () => null,
    bootstrap: async () => { refreshed = true; },
  });
  assert.equal(await bridge.addProject(), null);
  assert.equal(refreshed, false);
});

test('a rejected legacy API base URL update reaches its caller', async () => {
  // The setter remains for backward-compatible callers even though built-in
  // Provider settings no longer expose a custom endpoint editor.
  const bridge = bridgeWithHost({
    updateSettings: async () => { throw new Error('A turn is in progress.'); },
  });

  await assert.rejects(
    () => bridge.setApiBaseUrl('https://example.test'),
    /A turn is in progress\./,
    'capture must rethrow host update failures to every bridge caller',
  );
});

test('a rejected host updateSettings reaches the caller for the theme preference too', async () => {
  const bridge = bridgeWithHost({
    updateSettings: async () => { throw new Error('unsupported setting'); },
  });

  await assert.rejects(
    () => bridge.setThemePreference('system'),
    /unsupported setting/,
    'the same `capture` rethrow every inline settings error depends on',
  );
});

test('a successful host updateSettings resolves — the A/B for the two rejections above', async () => {
  const bridge = bridgeWithHost({
    updateSettings: async () => ({ version: 1, projects: [], pinnedSessions: [] }),
  });

  await bridge.setApiBaseUrl('https://example.test');
});

test('a rejected host updateSettings reaches the caller for voice preferences too', async () => {
  // Task 9 (desktop-audio-capability plan): the voice settings page relies
  // on the exact same rethrow — it has no dedicated inline error row of its
  // own for a voice-preference write and depends entirely on the shell's
  // global `<ErrorBanner>` to surface a failed write, which only happens if
  // `setVoicePreferences` actually rejects.
  const bridge = bridgeWithHost({
    updateSettings: async () => { throw new Error('unsupported setting'); },
  });

  await assert.rejects(
    () => bridge.setVoicePreferences(defaultVoicePreferences()),
    /unsupported setting/,
    'Voice.tsx has no inline error row; if this resolved instead of rejecting, a failed write would be reported nowhere at all',
  );
});

test('a successful host updateSettings resolves for voice preferences too', async () => {
  const bridge = bridgeWithHost({
    updateSettings: async () => ({ version: 1, projects: [], pinnedSessions: [], voice: defaultVoicePreferences() }),
  });

  await bridge.setVoicePreferences(defaultVoicePreferences());
});
