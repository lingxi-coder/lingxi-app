import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { ComputerUse } from '../../src/renderer/components/settings/pages/ComputerUse';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { ComputerSettingsCommand, ComputerSettingsSnapshot } from '../../src/shared/computerSettings';
import '../../src/renderer/components/settings/pages/ComputerUse.css';
let state: ComputerSettingsSnapshot = {
  revision: 'r0', enabled: true,
  apps: [{ bundleId: 'com.apple.finder', displayName: 'Finder', permission: 'allow' }, { bundleId: 'com.docker.docker', displayName: 'Docker', permission: 'deny' }],
  browser: { history: 'ask', siteTools: true, fullCdp: false, defaultBrowsing: 'ask', defaultDownloads: 'ask', sites: [] },
  browserStatus: { installed: false, connected: false, extensionDirectory: '/controlled/extension' },
  capabilities: { computer: true, browser: true, lockedUse: false, excel: false },
};
let revision = 0, failNext = '', holdNext = false;
let release: (() => void) | undefined;
const commands: ComputerSettingsCommand[] = [];
const listeners = new Set<(snapshot: ComputerSettingsSnapshot) => void>();
const copy = () => structuredClone(state);
const publish = () => { state = { ...state, revision: `r${++revision}` }; for (const listener of listeners) listener(copy()); };
Object.assign(window, { lingxi: {
  applicationIcon: { get: async () => null },
  computerSettings: {
    onChanged(listener: (snapshot: ComputerSettingsSnapshot) => void) { listeners.add(listener); return () => listeners.delete(listener); },
    async execute(command: ComputerSettingsCommand) {
      if (command.action === 'get') return copy();
      commands.push(structuredClone(command));
      if (holdNext) { holdNext = false; await new Promise<void>((resolve) => { release = resolve; }); }
      if (failNext) { const error = failNext; failNext = ''; throw new Error(error); }
      if ('revision' in command && command.revision !== state.revision) throw new Error('Settings changed; reload.');
      switch (command.action) {
        case 'setEnabled': state.enabled = command.enabled; break;
        case 'setApplication': {
          const existing = state.apps.find((app) => app.bundleId === command.bundleId);
          if (existing) existing.permission = command.permission;
          else state.apps.push({ bundleId: command.bundleId, displayName: 'Installed app', permission: command.permission });
          break;
        }
        case 'removeApplication': state.apps = state.apps.map((app) => app.bundleId === command.bundleId ? { ...app, permission: 'deny' } : app); break;
        case 'setBrowser': (state.browser as unknown as Record<string, unknown>)[command.key] = command.value; break;
        case 'setSite': {
          const url = new URL(command.origin);
          if (!['http:', 'https:'].includes(url.protocol) || command.origin !== url.origin) throw new Error('Use an exact HTTP or HTTPS origin.');
          state.browser.sites = state.browser.sites.filter((site) => site.origin !== command.origin);
          state.browser.sites.push({ origin: command.origin, browsing: command.browsing, downloads: command.downloads });
          break;
        }
        case 'removeSite': state.browser.sites = state.browser.sites.filter((site) => site.origin !== command.origin); break;
        case 'installBrowser': state.browserStatus.installed = true; break;
        case 'retryBrowser': state.browserStatus.connected = state.browserStatus.installed; break;
        case 'removeBrowser': state.browserStatus.installed = false; state.browserStatus.connected = false; break;
        case 'showExtension': break;
      }
      publish(); return copy();
    },
  },
}, computerSettingsFixture: {
  commands, hold() { holdNext = true; }, release() { release?.(); release = undefined; }, fail(message: string) { failNext = message; },
  external(enabled: boolean) { state.enabled = enabled; publish(); },
  unavailable() { state.error = 'Device settings are unreadable'; state.capabilities.computer = false; state.capabilities.browser = false; publish(); },
} });
function Fixture() {
  const [dark, setDark] = useState(false);
  Object.assign((window as unknown as { computerSettingsFixture: object }).computerSettingsFixture, { setDark });
  const t = tokens(dark);
  return <Theme.Provider value={t}><main className="settings-content-scroll" style={{ height: '100vh', overflow: 'auto', background: t.windowBg }}><div className="settings-content-inner settings-computer-use-content"><ComputerUse /></div></main></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
