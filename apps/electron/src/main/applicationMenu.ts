import type { MenuItemConstructorOptions } from 'electron';
import type { ApplicationMenuAction } from '../shared/applicationMenu.js';
import { nativeTr } from '../shared/nativeLocale.js';
import { botsAvailability } from './bots/service.js';

export function applicationMenuTemplate(
  platform: NodeJS.Platform,
  dispatch: (action: ApplicationMenuAction) => void,
  development: boolean,
): MenuItemConstructorOptions[] {
  const settings: MenuItemConstructorOptions = {
    label: nativeTr('native_settings'), accelerator: 'CommandOrControl+,', click: () => dispatch('settings'),
  };
  return [
    ...(platform === 'darwin' ? [{ role: 'appMenu' as const, submenu: [
      { role: 'about' as const }, { type: 'separator' as const }, settings,
      { type: 'separator' as const }, { role: 'services' as const },
      { type: 'separator' as const }, { role: 'hide' as const }, { role: 'hideOthers' as const },
      { role: 'unhide' as const }, { type: 'separator' as const }, { role: 'quit' as const },
    ] }] : [{ label: nativeTr('native_file'), submenu: [settings, { type: 'separator' as const }, { role: 'quit' as const }] }]),
    { role: 'editMenu' },
    { label: nativeTr('native_links'), submenu: [
      { label: nativeTr('native_open_link'), click: () => dispatch('open-link') },
    ] },
    { label: nativeTr('native_view'), submenu: [
      // What the sidebar lists: the same choice as the rail's workspace buttons. Bots stay in the menu where they cannot run, greyed out.
      { label: nativeTr('native_home'), accelerator: 'CommandOrControl+1', click: () => dispatch('home') },
      { label: nativeTr('native_bots'), accelerator: 'CommandOrControl+2', enabled: botsAvailability(platform).available, click: () => dispatch('bots') },
      { label: nativeTr('native_teams'), accelerator: 'CommandOrControl+3', click: () => dispatch('teams') },
      { label: nativeTr('native_apps'), accelerator: 'CommandOrControl+4', click: () => dispatch('apps') },
      { type: 'separator' },
      // The window alone knows whether the sidebar is shown, so one item stands for both Show and Hide.
      { label: nativeTr('native_toggle_sidebar'), accelerator: platform === 'darwin' ? 'Control+Command+S' : 'Control+Alt+S', click: () => dispatch('toggle-sidebar') },
      // The details of the open team, in Teams mode; the window ignores it elsewhere. ⌥⌘I is the DevTools role's own accelerator on macOS
      // (the development menu below has that role), so the Control key stands in for Option, as it does for the sidebar above.
      { label: nativeTr('native_team_details'), accelerator: platform === 'darwin' ? 'Control+Command+I' : 'Control+Alt+I', click: () => dispatch('team-details') },
      { label: nativeTr('native_activity'), accelerator: 'CommandOrControl+Alt+U', click: () => dispatch('activity') },
      { type: 'separator' }, { role: 'resetZoom' }, { role: 'zoomIn' }, { role: 'zoomOut' },
      { type: 'separator' }, { role: 'togglefullscreen' },
      ...(development ? [{ type: 'separator' as const }, { role: 'toggleDevTools' as const }] : []),
    ] },
    { role: 'windowMenu' },
  ];
}
