import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import { LOGIN_ITEMS_SETTINGS_URL, LoginItem } from '../src/main/bots/loginItem.js';
import type { BotsApi, BotsLoginItemUi } from '../src/shared/bots';
import { LoginItemController, loginItemView } from '../src/renderer/bridge/botLoginItem';
import { BotsBackground } from '../src/renderer/components/BotsBackground';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

// Match the existing render tests under the root tsconfig's classic JSX loader.
(globalThis as { React?: typeof React }).React = React;
pinEnglish();

// ---- The app as a login item ----

interface FakeSettings { openAtLogin: boolean; wasOpenedAtLogin?: boolean; status?: string }

/** The part of the app that the login item uses, with the system's state in `state` for the test to change behind its back. */
function fakeApp(isPackaged = true, initial: FakeSettings = { openAtLogin: false, wasOpenedAtLogin: false, status: 'not-registered' }) {
  const calls: unknown[][] = [];
  const state: FakeSettings = { ...initial };
  const app = {
    isPackaged,
    getLoginItemSettings: (options: unknown): FakeSettings => { calls.push(['get', options]); return { ...state }; },
    setLoginItemSettings: (settings: { openAtLogin: boolean; type: 'mainAppService' }): void => {
      calls.push(['set', settings]);
      state.openAtLogin = settings.openAtLogin;
      state.status = settings.openAtLogin ? 'enabled' : 'not-registered';
    },
  };
  return { app, state, calls };
}

function loginItem(fake: ReturnType<typeof fakeApp>, platform: NodeJS.Platform = 'darwin') {
  const opened: string[] = [];
  return { item: new LoginItem({ app: fake.app, platform, openExternal: async (url) => { opened.push(url); } }), opened };
}

test('only the installed app on a Mac can be a login item, and elsewhere the system is not even asked', () => {
  for (const [platform, packaged, available] of [['darwin', true, true], ['darwin', false, false], ['win32', true, false], ['linux', true, false]] as const) {
    const fake = fakeApp(packaged, { openAtLogin: true, wasOpenedAtLogin: true, status: 'enabled' });
    const { item } = loginItem(fake, platform);
    assert.deepEqual(item.status().available, available, `${platform} ${packaged ? 'installed' : 'built'}`);
    if (!available) {
      assert.deepEqual(item.status(), { available: false, enabled: false, needsApproval: false });
      assert.equal(item.openedInBackground(), false);
      assert.deepEqual(fake.calls, [], 'the system was not asked');
    }
  }
});

test('the system is asked how the item is, for the main app, each time', () => {
  const fake = fakeApp();
  const { item } = loginItem(fake);
  assert.deepEqual(item.status(), { available: true, enabled: false, needsApproval: false });
  assert.deepEqual(fake.calls, [['get', { type: 'mainAppService' }]]);
  // The owner turns it on in System Settings: nothing here remembers otherwise.
  fake.state.openAtLogin = true;
  fake.state.status = 'enabled';
  assert.deepEqual(item.status(), { available: true, enabled: true, needsApproval: false });
});

test('an item that waits for the owner to allow it is said to need approval, and is not enabled yet', () => {
  const fake = fakeApp(true, { openAtLogin: false, status: 'requires-approval' });
  assert.deepEqual(loginItem(fake).item.status(), { available: true, enabled: false, needsApproval: true });
  for (const status of ['not-registered', 'not-found', 'enabled', undefined]) {
    const other = fakeApp(true, { openAtLogin: false, ...(status !== undefined ? { status } : {}) });
    assert.equal(loginItem(other).item.status().needsApproval, false, String(status));
  }
});

test('turning the item on or off asks the system for the main app, and an app that cannot be one refuses', () => {
  const fake = fakeApp();
  const { item } = loginItem(fake);
  item.set(true);
  assert.deepEqual(item.status().enabled, true);
  item.set(false);
  assert.deepEqual(item.status().enabled, false);
  assert.deepEqual(fake.calls.filter(([kind]) => kind === 'set'), [
    ['set', { openAtLogin: true, type: 'mainAppService' }],
    ['set', { openAtLogin: false, type: 'mainAppService' }],
  ]);

  const built = fakeApp(false);
  assert.throws(() => loginItem(built).item.set(true), /available in the installed app/);
  assert.throws(() => loginItem(fakeApp(true), 'linux').item.set(true), /available in the installed app/);
  assert.deepEqual(built.calls, [], 'nothing was registered');
});

test('System Settings is opened at its Login Items pane and at no other place', async () => {
  const fake = fakeApp();
  const { item, opened } = loginItem(fake);
  await item.openSettings();
  assert.deepEqual(opened, ['x-apple.systempreferences:com.apple.LoginItems-Settings.extension']);
  assert.equal(LOGIN_ITEMS_SETTINGS_URL, opened[0]);
  // Whatever it is given, it opens that one.
  await (item as unknown as { openSettings(...args: unknown[]): Promise<void> }).openSettings('https://attacker.example');
  assert.deepEqual(opened, [LOGIN_ITEMS_SETTINGS_URL, LOGIN_ITEMS_SETTINGS_URL]);

  const refused = loginItem(fakeApp(false));
  assert.throws(() => refused.item.openSettings(), /available in the installed app/);
  assert.deepEqual(refused.opened, []);
});

test('the app waits in the background only when the system opened it at login, with the item on', () => {
  const cases: Array<[FakeSettings, boolean, string]> = [
    [{ openAtLogin: true, wasOpenedAtLogin: true, status: 'enabled' }, true, 'opened at login by the item'],
    [{ openAtLogin: true, wasOpenedAtLogin: false, status: 'enabled' }, false, 'the owner opened the app'],
    [{ openAtLogin: true, status: 'enabled' }, false, 'the system does not say'],
    // A Mac that opens the apps it had open at logout again says "at login" for them too.
    [{ openAtLogin: false, wasOpenedAtLogin: true, status: 'not-registered' }, false, 'reopened with the windows, the item is off'],
    [{ openAtLogin: false, wasOpenedAtLogin: true, status: 'requires-approval' }, false, 'the item is not allowed'],
  ];
  for (const [settings, expected, why] of cases) assert.equal(loginItem(fakeApp(true, settings)).item.openedInBackground(), expected, why);
});

// ---- What the page shows of it ----

const status = (enabled: boolean, needsApproval = false, available = true): BotsLoginItemUi => ({ available, enabled, needsApproval });

test('the switch is off and disabled where the app cannot be a login item, with the reason as its hint', () => {
  assert.deepEqual(loginItemView(status(false, false, false), false), { checked: false, disabled: true, note: 'unavailable', canOpenSettings: false });
  // Even when the system claims otherwise, an item that cannot be one is not shown as on.
  assert.deepEqual(loginItemView(status(true, false, false), false), { checked: false, disabled: true, note: 'unavailable', canOpenSettings: false });
});

test('until the system has answered the switch waits, off and disabled', () => {
  assert.deepEqual(loginItemView(undefined, false), { checked: false, disabled: true, note: 'default', canOpenSettings: false });
});

test('an item that is on is shown on, and one that waits for approval is shown on with the way to allow it', () => {
  assert.deepEqual(loginItemView(status(false), false), { checked: false, disabled: false, note: 'default', canOpenSettings: false });
  assert.deepEqual(loginItemView(status(true), false), { checked: true, disabled: false, note: 'default', canOpenSettings: false });
  assert.deepEqual(loginItemView(status(false, true), false), { checked: true, disabled: false, note: 'approval', canOpenSettings: true });
});

test('while a change is on its way the switch cannot be pressed again', () => {
  assert.equal(loginItemView(status(true), true).disabled, true);
  assert.equal(loginItemView(status(false), true).disabled, true);
  assert.equal(loginItemView(status(true), true).checked, true, 'and it still shows what it showed');
});

// ---- The controller ----

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

type LoginItemApi = BotsApi['loginItem'];

/** A host that answers from the state the test keeps, as the system would. */
function fakeApi(initial: BotsLoginItemUi) {
  const calls: unknown[][] = [];
  const state = { current: initial, settings: 0 };
  const api: LoginItemApi = {
    status: async () => { calls.push(['status']); return { ...state.current }; },
    set: async (enabled) => {
      calls.push(['set', enabled]);
      state.current = { ...state.current, enabled, needsApproval: false };
      return { ...state.current };
    },
    openSettings: async () => { calls.push(['openSettings']); state.settings += 1; },
  };
  return { api, state, calls };
}

test('without a host to ask the app is not one that can be a login item, and nothing is asked', async () => {
  const controller = new LoginItemController(undefined);
  assert.deepEqual(controller.getSnapshot(), { status: { available: false, enabled: false, needsApproval: false }, busy: false, error: undefined });
  await controller.refresh();
  await controller.setEnabled(true);
  await controller.openSettings();
  assert.equal(controller.getSnapshot().status?.available, false);
});

test('the status is unknown until the host has answered, and then it is what the host said', async () => {
  const fake = fakeApi(status(true));
  const controller = new LoginItemController(fake.api);
  assert.equal(controller.getSnapshot().status, undefined);
  await controller.refresh();
  assert.deepEqual(controller.getSnapshot(), { status: status(true), busy: false, error: undefined });
  // The owner changed it in System Settings meanwhile.
  fake.state.current = status(false);
  await controller.refresh();
  assert.deepEqual(controller.getSnapshot().status, status(false));
});

test('a status that could not be read is said, the last one stays, and the next read clears it', async () => {
  const fake = fakeApi(status(true));
  const controller = new LoginItemController(fake.api);
  await controller.refresh();
  fake.api.status = async () => { throw new Error("Error invoking remote method 'lingxi:bots:login-item:get': Error: The Bots folder cannot be read."); };
  await controller.refresh();
  assert.deepEqual(controller.getSnapshot(), { status: status(true), busy: false, error: 'The Bots folder cannot be read.' });
  fake.api.status = async () => status(false);
  await controller.refresh();
  assert.deepEqual(controller.getSnapshot(), { status: status(false), busy: false, error: undefined });
});

test('an answer that a later read overtook is dropped', async () => {
  const first = deferred<BotsLoginItemUi>();
  const second = deferred<BotsLoginItemUi>();
  const answers = [first, second];
  const api: LoginItemApi = { status: () => answers.shift()!.promise, set: async () => status(true), openSettings: async () => undefined };
  const controller = new LoginItemController(api);
  const early = controller.refresh();
  const late = controller.refresh();
  second.resolve(status(true));
  await late;
  first.resolve(status(false));
  await early;
  assert.deepEqual(controller.getSnapshot().status, status(true), 'the older answer did not undo the newer one');
});

test('turning the item on is asked of the host once at a time, and what the host answers is shown', async () => {
  const fake = fakeApi(status(false));
  const gate = deferred<void>();
  const real = fake.api.set;
  const sent: boolean[] = [];
  fake.api.set = (enabled) => { sent.push(enabled); return gate.promise.then(() => real(enabled)); };
  const controller = new LoginItemController(fake.api);
  await controller.refresh();
  const turning = controller.setEnabled(true);
  assert.equal(controller.getSnapshot().busy, true);
  await controller.setEnabled(false);
  assert.deepEqual(sent, [true], 'a second press while the first is on its way is not sent');
  gate.resolve();
  await turning;
  assert.deepEqual(controller.getSnapshot(), { status: status(true), busy: false, error: undefined });
});

test('an item that waits for approval is what turning it on came to, not a failure', async () => {
  const fake = fakeApi(status(false));
  fake.api.set = async () => status(false, true);
  const controller = new LoginItemController(fake.api);
  await controller.setEnabled(true);
  assert.deepEqual(controller.getSnapshot(), { status: status(false, true), busy: false, error: undefined });
});

test('a system that did not take the change is said so, in words that need no reason from the host', async () => {
  const fake = fakeApi(status(false));
  fake.api.set = async () => status(false);
  const controller = new LoginItemController(fake.api);
  await controller.setEnabled(true);
  assert.deepEqual(controller.getSnapshot(), { status: status(false), busy: false, error: '' });
  // Turning it off, when it stays on or still waits to be allowed, is no different.
  const stays = fakeApi(status(true));
  stays.api.set = async () => status(true);
  const staying = new LoginItemController(stays.api);
  await staying.setEnabled(false);
  assert.equal(staying.getSnapshot().error, '');
  const waits = fakeApi(status(false, true));
  waits.api.set = async () => status(false, true);
  const waiting = new LoginItemController(waits.api);
  await waiting.setEnabled(false);
  assert.equal(waiting.getSnapshot().error, '');
});

test('a change the host refused is said with its reason, and the switch is free again', async () => {
  const fake = fakeApi(status(false));
  fake.api.set = async () => { throw new Error("Error invoking remote method 'lingxi:bots:login-item:set': Error: Opening at login is available in the installed app."); };
  const controller = new LoginItemController(fake.api);
  await controller.refresh();
  await controller.setEnabled(true);
  assert.deepEqual(controller.getSnapshot(), { status: status(false), busy: false, error: 'Opening at login is available in the installed app.' });
  // Trying again clears the old complaint at once.
  fake.api.set = async () => status(true);
  const again = controller.setEnabled(true);
  assert.equal(controller.getSnapshot().error, undefined);
  await again;
  assert.deepEqual(controller.getSnapshot(), { status: status(true), busy: false, error: undefined });
});

test('a read that began during a change has the last word on the status, and the change is not lost', async () => {
  const change = deferred<BotsLoginItemUi>();
  const api: LoginItemApi = { status: async () => status(true), set: () => change.promise, openSettings: async () => undefined };
  const controller = new LoginItemController(api);
  const turning = controller.setEnabled(true);
  await controller.refresh();
  change.resolve(status(false));
  await turning;
  // The read came after the change was sent; its answer stands, and the refusal of the change is still said.
  assert.deepEqual(controller.getSnapshot(), { status: status(true), busy: false, error: '' });
});

test('System Settings is opened by asking the host, and a failure to is said', async () => {
  const fake = fakeApi(status(false, true));
  const controller = new LoginItemController(fake.api);
  await controller.openSettings();
  assert.deepEqual(fake.calls, [['openSettings']]);
  fake.api.openSettings = async () => { throw new Error('Error: The pane cannot be opened.'); };
  await controller.openSettings();
  assert.equal(controller.getSnapshot().error, 'The pane cannot be opened.');
});

// ---- The two settings, as the page shows them ----

function render(element: React.ReactElement): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
}

/** The section as it looks once the host has said how the login item is (or, with `read` off, before it has), and the owner has done `then`. */
async function background(
  api: LoginItemApi,
  { keepAwake = false, read = true, then }: { keepAwake?: boolean; read?: boolean; then?: (controller: LoginItemController) => Promise<void> } = {},
): Promise<string> {
  const controller = new LoginItemController(api);
  if (read) await controller.refresh();
  await then?.(controller);
  return render(React.createElement(BotsBackground, { keepAwake, onKeepAwake: () => undefined, loginItem: controller }));
}

const hostSays = (login: BotsLoginItemUi): LoginItemApi => fakeApi(login).api;

/** The attributes of the switch with this label. */
function switchOf(html: string, label: string): string {
  const match = new RegExp(`<button[^>]*role="switch"[^>]*aria-label="${label}"[^>]*>`).exec(html);
  assert.ok(match, `a switch labelled "${label}" in ${html}`);
  return match[0];
}

test('the page offers to keep the Mac awake, says what that does not do, and shows whether it is on', async () => {
  const off = await background(hostSays(status(false)));
  assert.match(off, /Running in the background/);
  assert.match(off, /Keep this Mac awake while Bots are working/);
  assert.match(off, /Also stays awake for the ten minutes before a Bot&#x27;s routine is due\. Closing the lid still puts the Mac to sleep\./);
  assert.match(switchOf(off, 'Keep this Mac awake while Bots are working'), /aria-checked="false"/);
  assert.match(switchOf(await background(hostSays(status(false)), { keepAwake: true }), 'Keep this Mac awake while Bots are working'), /aria-checked="true"/);
});

test('the page offers to open the app at login, in the background, and says what the Bots need of it', async () => {
  const html = await background(hostSays(status(false)));
  assert.match(html, /Open LingXi in the background when you log in/);
  assert.match(html, /Bots run routines and take tasks only while LingXi is open on this Mac\./);
  const login = switchOf(html, 'Open LingXi in the background when you log in');
  assert.match(login, /aria-checked="false"/);
  assert.ok(!login.includes('disabled'));
  assert.ok(!html.includes('Open Login Items'));
  assert.match(switchOf(await background(hostSays(status(true))), 'Open LingXi in the background when you log in'), /aria-checked="true"/);
});

test('where the app cannot be a login item its switch is disabled and says where it can be', async () => {
  const html = await background(hostSays(status(false, false, false)));
  assert.match(html, /Available in the installed app\./);
  assert.ok(!html.includes('Bots run routines and take tasks only while LingXi is open'));
  assert.match(switchOf(html, 'Open LingXi in the background when you log in'), /disabled/);
  // Keeping the Mac awake is not tied to that.
  assert.ok(!switchOf(html, 'Keep this Mac awake while Bots are working').includes('disabled'));
  // Nor is a window with no host to ask.
  assert.match(render(React.createElement(BotsBackground, { keepAwake: false, onKeepAwake: () => undefined, loginItem: new LoginItemController(undefined) })), /Available in the installed app\./);
});

test('an item that waits for approval is on, says where to allow it, and offers to open that place', async () => {
  const html = await background(hostSays(status(false, true)));
  assert.match(switchOf(html, 'Open LingXi in the background when you log in'), /aria-checked="true"/);
  assert.match(html, /Allow LingXi in System Settings → General → Login Items\./);
  assert.match(html, /<button type="button">Open Login Items<\/button>/);
});

test('a change the system did not take is said under the switch', async () => {
  const refused = fakeApi(status(false));
  refused.api.set = async () => status(false);
  assert.match(await background(refused.api, { then: (controller) => controller.setEnabled(true) }), /role="alert"[^>]*>The system did not accept the change\. Check System Settings → General → Login Items\./);
  const broken = fakeApi(status(false));
  broken.api.set = async () => { throw new Error('It broke.'); };
  assert.match(await background(broken.api, { then: (controller) => controller.setEnabled(true) }), /role="alert"[^>]*>The change could not be made: It broke\./);
  assert.ok(!(await background(hostSays(status(false)))).includes('role="alert"'));
});

test('before the system has answered the switch is there but cannot be pressed', async () => {
  assert.match(switchOf(await background(hostSays(status(false)), { read: false }), 'Open LingXi in the background when you log in'), /disabled/);
});
