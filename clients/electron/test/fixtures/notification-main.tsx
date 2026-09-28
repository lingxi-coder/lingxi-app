import { useCallback, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { ErrorBanner } from '../../src/renderer/components/ErrorBanner';
import { Notifications } from '../../src/renderer/components/settings/pages/Notifications';
import type { UseBridge } from '../../src/renderer/bridge/bridgeTypes.js';
import { defaultNotificationPreferences } from '../../src/shared/notificationPreferences';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

function Fixture() {
  const [dark, setDark] = useState(false);
  const [error, setError] = useState<string | null>('Provider credential missing: configure a trusted credential source');
  const [prefs, setPrefs] = useState(defaultNotificationPreferences);
  const [settings, setSettings] = useState(false);
  const clearError = useCallback(() => setError(null), []);
  const t = tokens(dark);
  Object.assign(window, { notificationFixture: { setDark, setError, setSettings } });
  const bridge = { error, clearError, bootstrap: { settings: { notifications: prefs } }, setNotificationPreferences: async (next) => setPrefs(next) } as UseBridge;
  return <Theme.Provider value={t}><main style={{ position: 'relative', height: '100vh', overflow: 'auto', background: t.stageBg, color: t.text, padding: '24px 32px' }}>
    {settings ? <div style={{ maxWidth: 660, margin: '0 auto' }}><Notifications bridge={bridge} /></div> : <>
      <header style={{ borderBottom: `1px solid ${t.border}`, paddingBottom: 14 }}>LingXi Code <span style={{ color: t.text3 }}> / Desktop</span></header>
      <div id="conversation" style={{ maxWidth: 640, margin: '130px auto', lineHeight: 1.8 }}><h2>继续你的工作</h2><p style={{ color: t.text2 }}>通知会在这里轻轻出现，聊天内容保持原位。</p></div>
      <ErrorBanner bridge={bridge} />
    </>}
  </main></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
