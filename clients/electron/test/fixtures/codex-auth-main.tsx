import { StrictMode, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import '../../src/renderer/global.css';
import { ProviderCredentials } from '../../src/renderer/components/settings/pages/ProviderCredentials';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';

function Fixture() {
  const [configured, setConfigured] = useState(false);
  const [open, setOpen] = useState(true);
  const [currentModel, setCurrentModel] = useState<string | null>(null);
  const [pendingModel, setPendingModel] = useState<string | undefined>();
  const calls = useRef({ login: 0, cancel: 0, logout: 0, apply: 0, close: 0 });
  const pending = useRef<{ resolve: (value: any) => void; reject: (error: Error) => void } | null>(null);
  const accountEmail = 'user@example.com';
  const credential = (isConfigured: boolean) => ({
    providerId: 'openai-chatgpt', configured: isConfigured, encryptionAvailable: true,
    ...(isConfigured ? { codexAccountEmail: accountEmail } : {}),
  });
  const metadata = credential(configured);
  const settings = { modelPickerVisibility: {} };
  const bridge = {
    bootstrap: { settings, providerCredentials: [metadata] },
    desktop: { currentModel, providerModelCatalog: [{ provider_id: 'openai-chatgpt', models: [
      { model_id: 'gpt-5.6-sol', reference: 'openai-chatgpt/gpt-5.6-sol', display_name: 'GPT-5.6 Sol' },
    ] }] },
    connected: true, running: false,
    loginCodex: () => { calls.current.login++; return new Promise((resolve, reject) => { pending.current = { resolve, reject }; }); },
    cancelCodexLogin: async () => {
      calls.current.cancel++;
      pending.current?.reject(new Error('登录已取消。'));
      pending.current = null;
    },
    clearProviderCredential: async (id: string) => {
      if (id !== 'openai-chatgpt') throw new Error('Unexpected provider');
      calls.current.logout++; setConfigured(false); return credential(false);
    },
    setModel: async (model: string) => { calls.current.apply++; setCurrentModel(model); },
    setModelPickerVisibility: async () => {},
  };
  (window as any).__codexAuthTest = {
    complete() { setConfigured(true); pending.current?.resolve({ credential: credential(true), settings }); pending.current = null; },
    fail() { pending.current?.reject(new Error('OAuth callback timed out')); pending.current = null; },
    close() { setOpen(false); },
    open(withModel = false) { setPendingModel(withModel ? 'openai-chatgpt/gpt-5.6-sol' : undefined); setOpen(true); },
    state() { return { ...calls.current, configured, currentModel, open }; },
  };
  return <Theme.Provider value={tokens(false)}>
    <main style={{ background: '#f7f7f8', minHeight: '100vh', padding: 40, boxSizing: 'border-box' }}>
      <section aria-label="Provider 设置" style={{ maxWidth: 780, margin: '0 auto' }}>
        {open && <ProviderCredentials bridge={bridge as any} initialProviderId="openai-chatgpt" pendingModelReference={pendingModel}
          snapshot={null} editingLayer="user" theme="light" onTheme={() => {}} onNavigate={() => {}} onJumpToLayer={() => {}}
          onClose={() => { calls.current.close++; setOpen(false); }} />}
      </section>
    </main>
  </Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<StrictMode><Fixture /></StrictMode>);
