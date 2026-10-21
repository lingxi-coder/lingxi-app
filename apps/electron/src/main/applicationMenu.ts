import type { MenuItemConstructorOptions } from 'electron';
import type { ApplicationMenuAction } from '../shared/applicationMenu.js';

export function applicationMenuTemplate(
  platform: string,
  dispatch: (action: ApplicationMenuAction) => void,
  development: boolean,
): MenuItemConstructorOptions[] {
  const settings: MenuItemConstructorOptions = {
    label: 'Settings…', accelerator: 'CommandOrControl+,', click: () => dispatch('settings'),
  };
  return [
    ...(platform === 'darwin' ? [{ role: 'appMenu' as const, submenu: [
      { role: 'about' as const }, { type: 'separator' as const }, settings,
      { type: 'separator' as const }, { role: 'services' as const },
      { type: 'separator' as const }, { role: 'hide' as const }, { role: 'hideOthers' as const },
      { role: 'unhide' as const }, { type: 'separator' as const }, { role: 'quit' as const },
    ] }] : [{ label: 'File', submenu: [settings, { type: 'separator' as const }, { role: 'quit' as const }] }]),
    { role: 'editMenu' },
    { label: 'Links', submenu: [
      { label: 'Open Link from Clipboard', click: () => dispatch('open-link') },
    ] },
    { label: 'View', submenu: [
      { label: 'Activity', accelerator: 'CommandOrControl+Alt+U', click: () => dispatch('activity') },
      { type: 'separator' }, { role: 'resetZoom' }, { role: 'zoomIn' }, { role: 'zoomOut' },
      { type: 'separator' }, { role: 'togglefullscreen' },
      ...(development ? [{ type: 'separator' as const }, { role: 'toggleDevTools' as const }] : []),
    ] },
    { role: 'windowMenu' },
  ];
}
