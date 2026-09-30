import { createRoot } from 'react-dom/client';
import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import '../../src/renderer/global.css';

// No real sessions, credentials, engine, or persistence.
const bridge = {
  activeSession: { sessionId: 'fixture', projectPath: '/fixture' },
  connected: true, sessionLoading: false, settingsSnapshotEvent: null,
  refreshSettingsSnapshot: async () => {},
  bootstrap: { settings: {}, workspace: {}, providerCredentials: [{ providerId: 'fixture', configured: true }] },
  desktop: { providerModelCatalog: [{ provider_id: 'fixture', provider_label: 'Configured provider',
    models: Array.from({ length: 72 }, (_, index) => ({ model_id: `model-${index}`, display_name: `Model ${String(index).padStart(2, '0')}`, fusion_analyst_capable: true })) }] },
  updateEngineSettings: async () => {},
};
createRoot(document.getElementById('root')!).render(<Theme.Provider value={tokens(false)}>
  <SettingsScreen bridge={bridge as never} theme="light" onTheme={() => {}} initialPageId="fusion" onClose={() => { throw new Error('Dropdown Escape must not close Settings'); }} />
</Theme.Provider>);
