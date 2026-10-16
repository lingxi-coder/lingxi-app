import assert from 'node:assert/strict';
import { test } from 'node:test';
import { applicationMenuTemplate } from '../src/main/applicationMenu.js';
import type { MenuItemConstructorOptions } from 'electron';

test('macOS menu exposes Settings and Activity with native editing and window roles', () => {
  const actions: string[] = [];
  const template = applicationMenuTemplate('darwin', action => actions.push(action), false);
  const appItems = template[0].submenu as MenuItemConstructorOptions[];
  const settings = appItems.find(item => item.label === 'Settings…')!;
  assert.equal(settings.accelerator, 'CommandOrControl+,');
  (settings.click as () => void)();
  const viewItems = template.find(item => item.label === 'View')!.submenu as MenuItemConstructorOptions[];
  (viewItems[0].click as () => void)();
  assert.deepEqual(actions, ['settings', 'activity']);
  assert.ok(appItems.some(item => item.role === 'services'));
  assert.ok(appItems.some(item => item.role === 'hide'));
  assert.ok(template.some(item => item.role === 'editMenu'));
  assert.ok(template.some(item => item.role === 'windowMenu'));
  assert.ok(!viewItems.some(item => item.role === 'toggleDevTools' || item.role === 'reload'));
});

test('other desktop platforms retain settings, editing, zoom, and quit', () => {
  const template = applicationMenuTemplate('win32', () => {}, true);
  assert.equal(template[0].label, 'File');
  assert.ok((template[0].submenu as MenuItemConstructorOptions[]).some(item => item.role === 'quit'));
  assert.ok((template.find(item => item.label === 'View')!.submenu as MenuItemConstructorOptions[]).some(item => item.role === 'toggleDevTools'));
});
