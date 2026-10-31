import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { AccountGate } from '../../src/renderer/components/AccountGate';
import { LingxiAccount } from '../../src/renderer/components/settings/pages/LingxiAccount';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { AccountApi, AccountState } from '../../src/shared/account';
import '../../src/renderer/global.css';

const user = { uid: 'fixture-owner', displayName: 'Fixture owner', email: null, emailVerified: true };
let state: AccountState = { user, busy: false, error: null, persistence: 'secure' };
const listeners = new Set<(state: AccountState) => void>();
const pending: Array<(state: AccountState) => void> = [];
const publish = (value: AccountState) => { state = value; for (const listener of listeners) listener(value); };
const fixture = {
  oauthCalls: 0,
  backgroundTabs: 0,
  expire: () => publish({ ...state, user: null, sessionExpired: true, error: 'Fixture session expired' }),
  timeout: () => publish({ ...state, busy: false, error: 'Fixture browser sign-in timed out' }),
  succeed: () => { publish({ user, busy: false, error: null, persistence: 'secure' }); pending[1](state); },
  finishOldReply: () => pending[0]({ user: null, sessionExpired: true, busy: false, error: 'Stale timeout', persistence: 'secure' }),
  signOut: () => publish({ user: null, busy: false, error: null, persistence: 'secure' }),
};
const account: AccountApi = {
  onChanged(callback) { listeners.add(callback); return () => listeners.delete(callback); },
  async request(command) {
    if (command.type === 'oauth') {
      fixture.oauthCalls++;
      publish({ ...state, busy: true, error: null });
      return new Promise(resolve => pending.push(resolve));
    }
    return state;
  },
};
(window as any).lingxi = { account };
(window as any).accountGateFixture = fixture;
// A still-mounted settings dialog must not steal the sign-in screen's Tab key.
document.addEventListener('keydown', event => {
  if (event.key === 'Tab') { fixture.backgroundTabs++; event.preventDefault(); }
});
function Workspace() {
  const [accountOpen, setAccountOpen] = useState(false);
  return <><nav aria-label="Workspace"><button onClick={() => setAccountOpen(true)}>Account</button></nav>
    <input aria-label="Workspace draft" defaultValue="Retain this local draft" />
    {accountOpen && <LingxiAccount />}</>;
}
createRoot(document.getElementById('root')!).render(<Theme.Provider value={tokens(false)}><AccountGate><Workspace /></AccountGate></Theme.Provider>);
