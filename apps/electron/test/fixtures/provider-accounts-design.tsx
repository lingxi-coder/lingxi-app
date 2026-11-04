import '../../src/renderer/global.css';
import '../../src/renderer/components/settings/pages/ProviderAccounts.css';
import { createRoot } from 'react-dom/client';
import { useState } from 'react';
import { ProviderCredentials } from '../../src/renderer/components/settings/pages/ProviderCredentials';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import { setLanguagePreference } from '../../src/renderer/i18n/useI18n';
import { fixtureAccountSnapshot } from './provider-accounts-mock';
import type { ProviderAccountCommand, ProviderAccountsApi } from '../../src/shared/providerAccounts';

// Entirely synthetic: this fixture never opens a browser or accesses native storage.
setLanguagePreference('zh-Hans');
let state = fixtureAccountSnapshot(['openai-chatgpt', 'openai']);
state.accounts[0].displayName = '日常使用'; state.accounts[1].displayName = '备用 API';
state.modelEntitlements = [{ accountId: state.accounts[0].accountId, identityGeneration: 1, modelId: 'gpt-5', status: 'confirmed', observedAtMs: 0 }, { accountId: state.accounts[1].accountId, identityGeneration: 1, modelId: 'gpt-5', status: 'forbidden', observedAtMs: 0 }];
let pending: { request: ProviderAccountCommand; resolve: () => void; reject: (error: Error) => void } | undefined;
let calls: ProviderAccountCommand[] = [];
let lastCredential: string | undefined;
let failSnapshot = false;
const api: ProviderAccountsApi = {
  snapshot: async () => { if (failSnapshot) throw new Error('Synthetic status failure'); return structuredClone(state); },
  cancel: async () => { pending?.reject(new Error('已取消登录')); pending = undefined; },
  command: async (request, credential) => {
    calls.push(request); lastCredential = credential;
    if (request.action === 'add' || request.action === 'reconnect' || request.action === 'test') await new Promise<void>((resolve, reject) => { pending = { request, resolve, reject }; });
    if (request.action === 'add') {
      const added = structuredClone(state.accounts[1]);
      added.accountId = crypto.randomUUID(); added.displayName = request.displayName!;
      added.authKind = request.authKind!; added.executionBindingId = request.executionBindingId!;
      added.userPriority = state.accounts.length; added.credentialRef.secretRef = `acct.${added.accountId}`;
      state.accounts.push(added);
      state.health.push({ ...state.health[1], accountId: added.accountId });
    }
    if (request.action === 'reorder') state.accounts.forEach(account => { account.userPriority = request.accountIds!.indexOf(account.accountId); });
    if (request.action === 'enable') state.accounts.find(account => account.accountId === request.accountId)!.enabled = request.enabled!;
    if (request.action === 'remove') state.accounts = state.accounts.filter(account => account.accountId !== request.accountId);
    state.configRevision++;
    return structuredClone(state);
  },
};
let show: (dark: boolean, directory?: boolean) => void;
const bridge: any = { bootstrap: { settings: {} }, desktop: { providerModelCatalog: [] }, providerAccounts: api,
  refreshModelPicker: async () => {}, providerSettings: {}, providerSettingsLocked: [], updateProviderSettings: async () => {} };
function Fixture() {
  const [dark, setDark] = useState(false), [directory, setDirectory] = useState(false);
  show = (next, list = false) => { setDark(next); setDirectory(list); };
  const t = tokens(dark);
  return <Theme.Provider value={t}><main style={{ background: t.appBg, color: t.text, fontFamily: '-apple-system, BlinkMacSystemFont, sans-serif', minHeight: '100vh', padding: '32px 40px', boxSizing: 'border-box' }}><ProviderCredentials key={String(directory)} {...({ bridge, initialProviderId: directory ? undefined : 'openai', onClose() {} } as any)} /></main></Theme.Provider>;
}
const until = async (predicate: () => boolean) => {
  const deadline = Date.now() + 4000;
  while (!predicate()) { if (Date.now() > deadline) throw new Error('UI state did not settle'); await new Promise(resolve => setTimeout(resolve, 20)); }
};
const click = (selector: string) => { const element = document.querySelector<HTMLElement>(selector); if (!element) throw new Error(`Missing ${selector}`); element.click(); };
(window as any).accountDesign = {
  ready: () => document.querySelectorAll('.pa-account').length === 2,
  show: async (dark: boolean, directory = false) => { show(dark, directory); await new Promise(resolve => setTimeout(resolve, 80)); },
  open: async () => { click('.pa-toolbar .pa-primary'); await until(() => Boolean(document.querySelector('.pa-inline-form'))); },
  run: async () => {
    const assert = (value: unknown, message: string) => { if (!value) throw new Error(message); };
    click('.pa-toolbar .pa-primary');
    await until(() => Boolean(document.querySelector('.pa-inline-form')));
    assert(document.activeElement === document.querySelector('.pa-inline-form button[type=submit]'), 'The primary authentication action receives focus');
    const bounds = document.querySelector('.pa-inline-form')!.getBoundingClientRect();
    assert(bounds.width <= 640 && bounds.x >= 0 && bounds.right <= innerWidth && !document.querySelector('dialog[open]'), 'Account entry stays inline without another modal layer');
    click('.pa-inline-form button[type=submit]');
    await until(() => Boolean(document.querySelector('.pa-progress')));
    assert(pending?.request.authKind === 'oauth', 'OAuth remains the default');
    assert(document.querySelector<HTMLButtonElement>('.pa-inline-form button[type=submit]')!.disabled, 'Duplicate login prevented');
    click('.pa-inline-form footer button[type=button]');
    await until(() => !document.querySelector('.pa-progress'));
    assert(document.querySelector('.pa-inline-form [role=alert]'), 'Cancellation feedback visible');
    click('.pa-inline-form footer button[type=button]');
    await until(() => !document.querySelector('.pa-inline-form'));
    assert(document.activeElement === document.querySelector('.pa-toolbar .pa-primary'), 'Focus returns to Add');
    click('.pa-toolbar .pa-primary');
    await until(() => Boolean(document.querySelector('.pa-inline-form')));
    click('.pa-segment button:last-child');
    await until(() => Boolean(document.querySelector('input[type=password]')));
    const secret = document.querySelector<HTMLInputElement>('input[type=password]')!;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(secret, 'synthetic-test-key');
    secret.dispatchEvent(new Event('input', { bubbles: true }));
    await until(() => !document.querySelector<HTMLButtonElement>('.pa-inline-form button[type=submit]')!.disabled);
    click('.pa-inline-form button[type=submit]'); await until(() => Boolean(pending));
    assert(lastCredential === 'synthetic-test-key' && pending?.request.authKind === 'api_key', 'Key uses credential channel');
    pending!.resolve(); pending = undefined;
    await until(() => !document.querySelector('.pa-inline-form'));
    assert(!document.querySelector('input[type=password]'), 'Secret field unmounted after save');
    assert(state.accounts.length === 3 && state.accounts.filter(a => a.authKind === 'oauth').length === 1 && state.accounts.filter(a => a.authKind === 'api_key').length === 2, 'Adding a key preserves existing OAuth and key accounts');
    assert(state.accounts[2].displayName === 'OpenAI', 'Optional name defaults to the provider');
    click('.pa-toolbar .pa-primary'); await until(() => Boolean(document.querySelector('.pa-inline-form')));
    click('.pa-segment button:last-child'); await until(() => Boolean(document.querySelector('input[type=password]')));
    assert(document.querySelector<HTMLInputElement>('input[type=password]')!.value === '', 'Reopening never retains the previous secret');
    click('.pa-inline-form footer button[type=button]'); await until(() => !document.querySelector('.pa-inline-form'));
    click('.pa-account:first-child .pa-test'); await until(() => Boolean(document.querySelector('.pa-test .pa-spinner')));
    pending!.resolve(); pending = undefined; await until(() => !document.querySelector('.pa-test .pa-spinner'));
    click('.pa-account:first-child .pa-reorder button:last-child');
    await until(() => document.querySelector<HTMLInputElement>('.pa-name')?.value === '备用 API');
    assert(calls.at(-1)?.action === 'reorder', 'Priority update issued');
    click('.pa-account:first-child .pa-more summary'); click('.pa-account:first-child .pa-destructive');
    await until(() => Boolean(document.querySelector('dialog[open]')));
    assert(calls.at(-1)?.action === 'reorder', 'Remove requires confirmation');
    click('dialog[open] footer button:first-child'); await until(() => !document.querySelector('dialog[open]'));
    await (window as any).accountDesign.show(false, true);
    const providerStatus = (id: string) => document.querySelector(`[data-testid="provider-list-item-${id}"] .provider-directory-status`);
    await until(() => providerStatus('openai')?.getAttribute('data-status') === 'configured');
    assert(providerStatus('openai')?.textContent?.includes('3 / 3'), 'Directory shows actual combined account count');
    assert(providerStatus('anthropic')?.getAttribute('data-status') === 'unconfigured', 'Only an empty account list is unconfigured');
    state.health.forEach(health => { health.status = 'needs_auth'; });
    click('.provider-directory-tools>button');
    await until(() => providerStatus('openai')?.getAttribute('data-status') === 'needs_auth');
    failSnapshot = true; click('.provider-directory-tools>button');
    await until(() => providerStatus('openai')?.getAttribute('data-status') === 'unknown');
    assert(providerStatus('openai')?.getAttribute('data-status') !== 'unconfigured', 'Read failures never mean unconfigured');
    failSnapshot = false;
    click('[data-testid="provider-list-item-anthropic"]');
    await until(() => Boolean(document.querySelector('.pa-inline-form input[type=password]')));
    assert(!document.querySelector('dialog[open]') && !document.querySelector('.pa-toolbar .pa-primary'), 'First account setup opens directly on the provider page');
    assert(document.querySelector<HTMLAnchorElement>('.pa-key-link a')?.href === 'https://console.anthropic.com/settings/keys', 'Key management follows the selected provider');
    return { initialSetup: true, multiAccount: true, directoryStatuses: true, inline: true, focus: true, oauthCancel: true, apiKeyClear: true, testingFeedback: true, reorder: true, removalConfirmation: true };
  },
};
createRoot(document.getElementById('root')!).render(<Fixture />);
