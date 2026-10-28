import { createRoot } from 'react-dom/client';
import { DesktopUpdateReminder } from '../../src/renderer/components/DesktopUpdateReminder';
import type { DesktopUpdateState, DesktopUpdatesApi } from '../../src/shared/desktopUpdates';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

const dark = new URLSearchParams(location.search).get('theme') === 'dark';
const t = tokens(dark);
const listeners = new Set<(state: DesktopUpdateState) => void>();
let state: DesktopUpdateState = { currentVersion: '0.1.0', release: null };
let fail = false;
let downloads = 0;
const api: DesktopUpdatesApi = {
  getState: async () => state,
  onChanged(listener) { listeners.add(listener); return () => { listeners.delete(listener); }; },
  download: async () => { if (fail) throw new Error('browser failed'); downloads++; },
  remindLater: async () => { state = { ...state, release: null }; listeners.forEach(listener => listener(state)); },
};
(window as any).lingxi = { desktopUpdates: api };
const root = createRoot(document.getElementById('root')!);
(window as any).updateFixture = {
  show() { state = { ...state, release: { version: '0.2.0', downloadUrl: 'https://lingxios.net/packages/update.zip' } }; listeners.forEach(listener => listener(state)); },
  fail(value: boolean) { fail = value; },
  downloads: () => downloads,
  listeners: () => listeners.size,
  unmount: () => root.unmount(),
};
localStorage.setItem('lingxi.desktop.language', 'zh-Hans');
root.render(<Theme.Provider value={t}>
  <div className="desktop-navigation" style={{ width: '100%', background: t.sidebarBg }}>
    <div className="desktop-body">
    <nav className="desktop-nav-rail" style={{ '--text3': t.text3 } as React.CSSProperties}>
      <span aria-hidden="true" style={{ flex: 1 }} />
      <DesktopUpdateReminder />
    </nav>
    </div>
    <button id="outside" style={{ position: 'absolute', top: 30, left: 100 }}>Outside</button>
  </div>
</Theme.Provider>);
