import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToString } from 'react-dom/server';

import { useBridge, type UseBridge } from '../src/renderer/bridge/useBridge';

/**
 * Whether a failing host IPC call reaches the page that asked for it.
 *
 * Every settings page renders its own inline error by hanging a `.catch()`
 * off a `bridge.*` method — `ProviderCredentials`' `apiBaseUrlError` row,
 * `Plugins`' `saveError`, `Projects`' `error`, `RawJson`'s `saveError`. All
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

test('a rejected host updateSettings reaches the caller, so the API base URL row can show it', async () => {
  // The scenario: a turn is in flight and the user edits the custom API base
  // URL. `host.ts`'s `assertNoActiveTurn` throws, because changing
  // `apiBaseUrl` restarts the bridge.
  const bridge = bridgeWithHost({
    updateSettings: async () => { throw new Error('A turn is in progress.'); },
  });

  await assert.rejects(
    () => bridge.setApiBaseUrl('https://example.test'),
    /A turn is in progress\./,
    'ProviderCredentials renders its apiBaseUrlError from a `.catch()` on this promise; '
    + 'if it resolves, the row silently shows nothing and the save silently does nothing',
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
