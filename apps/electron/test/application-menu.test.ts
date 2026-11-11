import assert from 'node:assert/strict';
import { test } from 'node:test';
import { applicationMenuTemplate } from '../src/main/applicationMenu.js';
import { nativeCatalog } from '../src/shared/nativeCatalog.js';
import type { MenuItemConstructorOptions } from 'electron';

const viewItems = (template: MenuItemConstructorOptions[]) => template.find(item => item.label === 'View')!.submenu as MenuItemConstructorOptions[];

test('macOS menu exposes Settings and Activity with native editing and window roles', () => {
  const actions: string[] = [];
  const template = applicationMenuTemplate('darwin', action => actions.push(action), false);
  const appItems = template[0].submenu as MenuItemConstructorOptions[];
  const settings = appItems.find(item => item.label === 'Settings…')!;
  assert.equal(settings.accelerator, 'CommandOrControl+,');
  (settings.click as () => void)();
  const view = viewItems(template);
  assert.equal(view.find(item => item.label === 'Apps')?.accelerator, 'CommandOrControl+4');
  (view.find(item => item.label === 'Activity')!.click as () => void)();
  assert.deepEqual(actions, ['settings', 'activity']);
  assert.ok(appItems.some(item => item.role === 'services'));
  assert.ok(appItems.some(item => item.role === 'hide'));
  assert.ok(template.some(item => item.role === 'editMenu'));
  assert.ok(template.some(item => item.role === 'windowMenu'));
  assert.ok(!view.some(item => item.role === 'toggleDevTools' || item.role === 'reload'));
});

test('other desktop platforms retain settings, editing, zoom, and quit', () => {
  const template = applicationMenuTemplate('win32', () => {}, true);
  assert.equal(template[0].label, 'File');
  assert.ok((template[0].submenu as MenuItemConstructorOptions[]).some(item => item.role === 'quit'));
  assert.ok(viewItems(template).some(item => item.role === 'toggleDevTools'));
});

test('the View menu picks what the sidebar lists and shows or hides it, each item sending its own action', () => {
  const actions: string[] = [];
  const view = viewItems(applicationMenuTemplate('darwin', action => actions.push(action), false));
  const labelled = (label: string) => view.find(item => item.label === label)!;
  assert.deepEqual(['Home', 'Bots', 'Teams', 'Apps', 'Friends'].map(label => labelled(label).accelerator), ['CommandOrControl+1', 'CommandOrControl+2', 'CommandOrControl+3', 'CommandOrControl+4', 'CommandOrControl+5']);
  assert.equal(labelled('Show/Hide Sidebar').accelerator, 'Control+Command+S');
  for (const label of ['Home', 'Bots', 'Teams', 'Friends', 'Show/Hide Sidebar']) (labelled(label).click as () => void)();
  assert.deepEqual(actions, ['home', 'bots', 'teams', 'friends', 'toggle-sidebar']);
  assert.ok(view.findIndex(item => item.label === 'Friends') < view.findIndex(item => item.label === 'Show/Hide Sidebar'), 'Friends is one of the modes');
  assert.ok(view.findIndex(item => item.label === 'Teams') < view.findIndex(item => item.label === 'Show/Hide Sidebar'), 'the modes come before the sidebar toggle');
});

test('Bots are greyed out in the menu where they cannot run, not left out of it', () => {
  for (const [platform, enabled] of [['darwin', true], ['win32', false], ['linux', false]] as const) {
    const bots = viewItems(applicationMenuTemplate(platform, () => {}, false)).find(item => item.label === 'Bots');
    assert.ok(bots, `${platform} still lists Bots`);
    assert.equal(bots.enabled, enabled, platform);
  }
});

test('the sidebar toggle has an accelerator that does not need the Command key off macOS', () => {
  const toggle = viewItems(applicationMenuTemplate('linux', () => {}, false)).find(item => item.label === 'Show/Hide Sidebar')!;
  assert.equal(toggle.accelerator, 'Control+Alt+S');
});

test('the View menu shows or hides the details of the open team, after the sidebar toggle, sending its own action', () => {
  const actions: string[] = [];
  const view = viewItems(applicationMenuTemplate('darwin', action => actions.push(action), false));
  const details = view.find(item => item.label === 'Show/Hide Team Details')!;
  assert.ok(details, 'the item is in the menu');
  assert.equal(details.accelerator, 'Control+Command+I');
  (details.click as () => void)();
  assert.deepEqual(actions, ['team-details']);
  assert.ok(view.findIndex(item => item.label === 'Show/Hide Sidebar') < view.indexOf(details), 'it follows the sidebar toggle');
  for (const platform of ['win32', 'linux'] as const) {
    const other = viewItems(applicationMenuTemplate(platform, () => {}, false)).find(item => item.label === 'Show/Hide Team Details')!;
    assert.equal(other.accelerator, 'Control+Alt+I', `${platform} does not need the Command key`);
  }
});

/** What Electron gives the roles of the View menu when the template names no accelerator (lib/browser/api/menu-item-roles.ts). */
const roleAccelerators = (platform: 'darwin' | 'win32' | 'linux'): Record<string, string> => ({
  resetZoom: 'CommandOrControl+0', zoomIn: 'CommandOrControl+Plus', zoomOut: 'CommandOrControl+-',
  togglefullscreen: platform === 'darwin' ? 'Control+Command+F' : 'F11',
  toggleDevTools: platform === 'darwin' ? 'Alt+Command+I' : 'Control+Shift+I',
});

/** An accelerator spelled one way: the modifiers by their platform names, in order, then the key. */
const canonicalAccelerator = (accelerator: string, platform: string): string => {
  const names: Record<string, string> = { commandorcontrol: platform === 'darwin' ? 'command' : 'control', cmdorctrl: platform === 'darwin' ? 'command' : 'control', cmd: 'command', ctrl: 'control', option: 'alt' };
  const parts = accelerator.split('+').map(part => names[part.toLowerCase()] ?? part.toLowerCase());
  return [...parts.slice(0, -1).sort(), parts.at(-1)].join('+');
};

test('no two items of the View menu share an accelerator, the DevTools role of the development menu among them', () => {
  for (const platform of ['darwin', 'win32', 'linux'] as const) {
    const roles = roleAccelerators(platform);
    const taken = new Map<string, string>();
    for (const item of viewItems(applicationMenuTemplate(platform, () => {}, true))) {
      const accelerator = item.accelerator ?? (item.role ? roles[item.role] : undefined);
      if (!accelerator) continue;
      const key = canonicalAccelerator(String(accelerator), platform);
      const name = item.label ?? item.role ?? '';
      assert.ok(!taken.has(key), `${platform}: ${name} and ${taken.get(key)} both use ${accelerator}`);
      taken.set(key, name);
    }
    assert.ok(taken.has(canonicalAccelerator(roles['toggleDevTools']!, platform)), `${platform}: the DevTools role is in the development menu, so the check saw it`);
  }
});

test('the new View items are named in all five languages', () => {
  for (const key of ['native_home', 'native_bots', 'native_teams', 'native_friends', 'native_toggle_sidebar', 'native_team_details']) {
    for (const locale of ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko']) assert.ok(nativeCatalog[key]?.[locale]?.trim(), `${key} ${locale}`);
  }
});
