import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  browserProbeDeps,
  hostMicrophonePermissionReader,
  probePlatform,
  subscribeMicrophoneGrantChanges,
  type VoicePermissionStatus,
} from '../src/renderer/audio/capabilities';
import { voicePageModel } from '../src/renderer/components/settings/pages/Voice';
import { defaultVoicePreferences } from '../src/shared/voicePreferences';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';

/**
 * Final review, Defect 7/9: the voice page's 麦克风权限 row reported a value
 * that could not reflect the macOS microphone grant.
 *
 * `queryBrowserMicrophonePermission` asked
 * `navigator.permissions.query({name:'microphone'})`, which in this app is
 * answered by `main/index.ts`'s `session.setPermissionCheckHandler` —
 * `permission === 'media' && isLocalRendererUrl(origin)`, i.e. always `true`
 * for the app's own renderer, computed with no reference to macOS TCC. The
 * reviewer measured it on real Electron 43 with this app's exact handlers:
 * `{"permissionsApiState":"granted","macOsTccStatus":"not-determined"}`.
 *
 * So the row said 已授权 while every recording failed, the banner claimed
 * 「录音与语音朗读功能不受影响」, the Status card said 没有需要额外说明的问题,
 * and `microphoneActionable` (`=== 'denied'`) was permanently false — which
 * made the 「打开系统设置」 button, `bridge.openSystemSettings('microphone')`
 * and the `microphone` entry in `SYSTEM_SETTINGS_PANES` unreachable code.
 *
 * The authoritative source on macOS is the MAIN process:
 * `systemPreferences.getMediaAccessStatus('microphone')` returns the real TCC
 * grant. These tests pin that the answer comes from there and reaches the row.
 * The end-to-end proof, in a real Electron renderer against the real OS, is
 * `microphone-permission-electron.test.mjs`.
 */

/** The wire name of the channel — kept as a literal here on purpose: it is a contract between `main/host.ts` and `preload/index.ts`, and a shared constant would let both drift together. */
const CH_MICROPHONE_ACCESS_GET = 'lingxi:microphone-access:get';

interface HostHarness {
  handlers: Map<string, (...args: unknown[]) => unknown>;
  event: unknown;
  dispose(): void;
}

/** A `HostController` with stubbed IPC and a stubbed OS media-access reader — the same shape `host.test.ts` builds. */
function hostHarness(mediaAccess?: { getMediaAccessStatus(mediaType: string): string }): HostHarness {
  const userData = mkdtempSync(join(tmpdir(), 'lingxi-mic-permission-settings-'));
  const projectDirectory = mkdtempSync(join(tmpdir(), 'lingxi-mic-permission-project-'));
  const project = realpathSync.native(projectDirectory);
  const settings = new SettingsStore(userData);
  settings.addProject(project);
  settings.activateProject(project);

  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const ipc = {
    handle: (channel: string, handler: (...args: unknown[]) => unknown) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  };
  const bridge = { registerIpc: () => undefined, registerWindow: () => undefined, get: () => undefined };
  const host = new HostController(
    settings,
    bridge as never,
    new DiagnosticBuffer(),
    undefined,
    ipc as never,
    mediaAccess as never,
  );
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once: () => undefined, removeListener: () => undefined };
  host.registerWindow(sender as never, frame.url);
  host.registerIpc();
  return {
    handlers,
    event: { sender, senderFrame: frame },
    dispose: () => {
      rmSync(userData, { recursive: true, force: true });
      rmSync(projectDirectory, { recursive: true, force: true });
    },
  };
}

test('the main process answers the microphone-permission channel by asking the OS for the microphone grant', async () => {
  const calls: string[] = [];
  const harness = hostHarness({
    getMediaAccessStatus: (mediaType: string) => { calls.push(mediaType); return 'denied'; },
  });
  try {
    const handler = harness.handlers.get(CH_MICROPHONE_ACCESS_GET);
    assert.ok(
      handler,
      `the main process registers no ${CH_MICROPHONE_ACCESS_GET} handler, so the renderer has no way to read the real `
      + 'macOS microphone grant and the 麦克风权限 row cannot be anything but a guess',
    );
    const status = await handler(harness.event);
    assert.deepEqual(
      calls,
      ['microphone'],
      'the handler must read systemPreferences.getMediaAccessStatus("microphone") — the only API that reports the OS TCC grant',
    );
    assert.equal(status, 'denied');
  } finally {
    harness.dispose();
  }
});

test('every macOS media-access status maps to an honest row state, and an unknown one is never reported as granted', async () => {
  const cases: [string, VoicePermissionStatus][] = [
    ['granted', 'granted'],
    ['denied', 'denied'],
    // "the user has not been asked yet" — distinct from denied: there is
    // nothing for 「打开系统设置」 to fix, the OS prompt has yet to happen.
    ['not-determined', 'prompt'],
    // MDM/parental controls: the grant cannot be given, and recording fails
    // exactly as it does for `denied`. Reported as denied rather than as a
    // fourth label the page has no honest sentence for.
    ['restricted', 'denied'],
    // Electron's own "I cannot tell" answer, plus anything a future Electron
    // adds: the row says 无法确定, never 已授权.
    ['unknown', 'unavailable'],
    ['something-electron-has-not-invented-yet', 'unavailable'],
  ];
  for (const [raw, expected] of cases) {
    const harness = hostHarness({ getMediaAccessStatus: () => raw });
    try {
      const handler = harness.handlers.get(CH_MICROPHONE_ACCESS_GET);
      assert.ok(handler, `no ${CH_MICROPHONE_ACCESS_GET} handler is registered`);
      assert.equal(await handler(harness.event), expected, `macOS "${raw}" must be reported as "${expected}"`);
    } finally {
      harness.dispose();
    }
  }
});

test('a machine with no media-access API at all reports "unavailable", never a constant "granted"', async () => {
  // Linux Electron has no `getMediaAccessStatus` at all. "I cannot tell" is a
  // real state the row can say (无法确定); claiming a grant nobody checked is
  // the defect this whole file exists to remove.
  const harness = hostHarness(undefined);
  try {
    const handler = harness.handlers.get(CH_MICROPHONE_ACCESS_GET);
    assert.ok(handler, `no ${CH_MICROPHONE_ACCESS_GET} handler is registered`);
    // Outside Electron `require('electron')` is the binary PATH string, so the
    // production default reader is absent here — the same shape as Linux.
    assert.equal(await handler(harness.event), 'unavailable');
  } finally {
    harness.dispose();
  }
});

/** Installs the browser globals `browserProbeDeps` touches, with a `navigator.permissions` that lies exactly the way this app's own permission-check handler makes it lie. */
function installBrowserGlobals(): () => void {
  const hadWindow = 'window' in globalThis;
  const previousWindow = (globalThis as { window?: unknown }).window;
  const previousNavigator = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
  (globalThis as { window?: unknown }).window = {
    speechSynthesis: {
      getVoices: () => [],
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    },
  };
  Object.defineProperty(globalThis, 'navigator', {
    configurable: true,
    value: {
      language: 'zh-CN',
      // What `session.setPermissionCheckHandler` makes this answer for the
      // app's own renderer, whatever macOS actually thinks.
      permissions: { query: async () => ({ state: 'granted' }) },
    },
  });
  return () => {
    if (hadWindow) (globalThis as { window?: unknown }).window = previousWindow;
    else delete (globalThis as { window?: unknown }).window;
    if (previousNavigator) Object.defineProperty(globalThis, 'navigator', previousNavigator);
    else delete (globalThis as { navigator?: unknown }).navigator;
  };
}

test('the voice page probe takes the microphone state from the OS-backed host bridge, never from navigator.permissions', async () => {
  const restore = installBrowserGlobals();
  try {
    const deps = browserProbeDeps(null, [], async () => 'denied');
    assert.equal(
      await deps.queryMicrophonePermission(),
      'denied',
      'the probe answered from navigator.permissions (the page permission this app grants itself), not from the OS grant '
      + 'the main process reads — the exact substitution that let the row say 已授权 while every recording failed',
    );
  } finally {
    restore();
  }
});

test('a denied OS grant reaches the 麦克风权限 row, the blocking issue, the banner and the System Settings button', async () => {
  const snapshot = await probePlatform({
    synth: { getVoices: () => [], addEventListener: () => undefined, removeEventListener: () => undefined },
    queryMicrophonePermission: async () => 'denied',
    localeTag: 'zh-CN',
    providerConfigured: true,
    providerTranscriptionCapable: false,
    voiceListTimeoutMs: 5,
  });
  const model = voicePageModel(defaultVoicePreferences(), snapshot);

  assert.equal(model.microphonePermission, 'denied');
  assert.equal(model.microphonePermissionLabel, '未授权');
  // `Voice.tsx` renders the 「打开系统设置」 button — the only control that
  // opens `SYSTEM_SETTINGS_PANES.microphone` — under exactly this flag.
  assert.equal(model.microphoneActionable, true, 'the System Settings deep link is unreachable while this is false');
  assert.ok(
    model.notices.includes('尚未获得麦克风权限，录音功能无法使用。'),
    'MicrophonePermissionRequired must reach the Status card',
  );
  assert.ok(model.recognitionUnavailableNotice.includes('尚未获得麦克风权限，录音功能无法使用。'));
  assert.ok(
    !model.recognitionUnavailableNotice.includes('录音与语音朗读功能不受影响'),
    'the banner must not claim recording is unaffected while the OS denies the microphone',
  );
});

test('the microphone System Settings pane is accepted by the main process, so the row\'s button is not a dead link', async () => {
  const harness = hostHarness();
  try {
    const handler = harness.handlers.get('lingxi:openSystemSettings');
    assert.ok(handler, 'no openSystemSettings handler is registered');
    await assert.rejects(
      () => Promise.resolve(handler(harness.event, 'not-a-pane')),
      /unsupported System Settings pane/,
      'the allowlist check must reject an unknown pane — otherwise the assertion below proves nothing',
    );
    // Outside Electron `shell` is undefined, so the accepted path fails LATER,
    // in `shell.openExternal` — which is itself the proof that `'microphone'`
    // passed the allowlist instead of being rejected as unsupported.
    await assert.rejects(
      () => Promise.resolve(handler(harness.event, 'microphone')),
      (error: Error) => !/unsupported System Settings pane/.test(String(error.message)),
      '"microphone" must be an allowed System Settings pane',
    );
  } finally {
    harness.dispose();
  }
});

/** A recording `EventTarget` stand-in for `window`/`document`. */
function listenerTarget(visibilityState?: string) {
  const listeners = new Map<string, Set<() => void>>();
  return {
    visibilityState,
    addEventListener: (type: string, listener: () => void) => {
      const set = listeners.get(type) ?? new Set<() => void>();
      set.add(listener);
      listeners.set(type, set);
    },
    removeEventListener: (type: string, listener: () => void) => { listeners.get(type)?.delete(listener); },
    emit: (type: string) => { for (const listener of [...(listeners.get(type) ?? [])]) listener(); },
    count: (type: string) => listeners.get(type)?.size ?? 0,
  };
}

test('the microphone grant is re-read when the user comes back, not read once and trusted forever', () => {
  // The user can flip the grant in System Settings — usually because this very
  // page told them to — and macOS emits no event for it. A row that reads the
  // grant once at mount is wrong from that moment on.
  const windowTarget = listenerTarget();
  const documentTarget = listenerTarget('visible');
  let refreshes = 0;
  const unsubscribe = subscribeMicrophoneGrantChanges(() => { refreshes += 1; }, { window: windowTarget, document: documentTarget });

  windowTarget.emit('focus');
  assert.equal(refreshes, 1, 'returning to the window must re-read the grant');
  documentTarget.emit('visibilitychange');
  assert.equal(refreshes, 2, 'a window revealed without taking focus must re-read the grant too');

  documentTarget.visibilityState = 'hidden';
  documentTarget.emit('visibilitychange');
  assert.equal(refreshes, 2, 'leaving the window is not a moment to spend an IPC round trip on');

  unsubscribe();
  windowTarget.emit('focus');
  documentTarget.visibilityState = 'visible';
  documentTarget.emit('visibilitychange');
  assert.equal(refreshes, 2, 'the subscription must be released with the page');
  assert.equal(windowTarget.count('focus'), 0);
  assert.equal(documentTarget.count('visibilitychange'), 0);
});

test('an unreachable or nonsense host answer is "unavailable", never an assumed grant', async () => {
  assert.equal(await hostMicrophonePermissionReader(undefined)(), 'unavailable');
  assert.equal(await hostMicrophonePermissionReader({} as never)(), 'unavailable');
  assert.equal(await hostMicrophonePermissionReader({ microphoneAccess: async () => 'yes-please' })(), 'unavailable');
  assert.equal(await hostMicrophonePermissionReader({ microphoneAccess: async () => undefined })(), 'unavailable');
  assert.equal(await hostMicrophonePermissionReader({ microphoneAccess: async () => 'denied' })(), 'denied');
  assert.equal(await hostMicrophonePermissionReader({ microphoneAccess: async () => 'granted' })(), 'granted');
});

const repositoryRoot = join(import.meta.dirname, '../../..');

/** Every renderer source file, as `{ path, source }`. */
function rendererSources(): { path: string; source: string }[] {
  const paths = execFileSync('git', ['ls-files', '--', 'clients/electron/src/renderer'],
    { cwd: repositoryRoot, encoding: 'utf8' })
    .trim().split('\n')
    .filter((path) => path.endsWith('.ts') || path.endsWith('.tsx'));
  return paths.map((path) => ({ path, source: readFileSync(join(repositoryRoot, path), 'utf8') }));
}

/**
 * Strips comments before scanning, the same way
 * `voice-capability-snapshot.test.ts`'s forbidden-copy guard does and for the
 * same reason: `capabilities.ts` and `useBridge.ts` legitimately NAME
 * `navigator.permissions` in doc comments explaining why it must never be the
 * source. The guard has to fire on the expression reappearing in real CODE,
 * not on prose warning against it.
 */
function withoutComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:'"])\/\/.*$/gm, '$1');
}

test('guard sanity: the renderer scanner really reads renderer code, and comment-stripping really strips', () => {
  const sources = rendererSources();
  assert.ok(sources.length > 10, `the renderer scanner found only ${sources.length} files — every zero below would prove nothing`);
  assert.ok(
    sources.some(({ source }) => withoutComments(source).includes('navigator.language')),
    'a known-present renderer expression survived stripping — if it did not, the guard below would be testing nothing',
  );
  assert.ok(
    sources.some(({ source }) => source.includes('navigator.permissions')),
    'the doc comments explaining why the Permissions API is forbidden are gone — if that wording changes, this guard needs re-checking against a real positive case',
  );
});

test('guard: no renderer code answers the microphone question from the Permissions API', () => {
  const offenders = rendererSources()
    .filter(({ source }) => withoutComments(source).includes('navigator.permissions'))
    .map(({ path }) => path);
  assert.deepEqual(
    offenders,
    [],
    'navigator.permissions reports the PAGE permission, which this app grants itself in '
    + 'main/index.ts\'s setPermissionCheckHandler with no reference to the OS grant. It can say granted while macOS '
    + 'denies the microphone, which is how the 麦克风权限 row came to lie; the only honest source is the main '
    + 'process\'s systemPreferences.getMediaAccessStatus, reached through useBridge\'s microphonePermission()',
  );
});
