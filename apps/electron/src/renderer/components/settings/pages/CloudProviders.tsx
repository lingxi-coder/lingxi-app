import { useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import type { AccountApi } from '../../../../shared/account';
import type { ProviderVaultApi, ProviderVaultReply, VaultProvider } from '../../../../shared/providerVault';
import { DESKTOP_BRIDGE_GLOBAL } from '../../../../shared/desktopBridge';
import { useAccountState } from '../../../bridge/useAccountState';
import { tr } from '../../../i18n/locale';
import { useI18n } from '../../../i18n/useI18n';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';

const SYNC_PREFERENCE = 'lingxi.providerVault.sync';
/** Whether the last add asked for cloud sync; the choice is remembered on this Mac only. */
export function readVaultSyncPreference(): boolean {
  try { return globalThis.localStorage?.getItem(SYNC_PREFERENCE) === '1'; } catch { return false; }
}
export function writeVaultSyncPreference(value: boolean): void {
  try { globalThis.localStorage?.setItem(SYNC_PREFERENCE, value ? '1' : '0'); } catch { /* the choice is a convenience */ }
}

/** A vault error code as a short phrase; a code without one says something went wrong and names the code. */
export function vaultCodeText(code: string | undefined): string {
  switch (code) {
    case 'NETWORK': case 'UNAVAILABLE': case 'TIMEOUT': return tr('provider_vault_code_network');
    case 'UNSUPPORTED_PROVIDER': case 'NO_MODELS': case 'PROVIDER_INCOMPLETE': case 'INVALID_INPUT': case 'KEY_REQUIRED': case 'UNKNOWN_PRESET': return tr('provider_vault_code_unsupported');
    case 'PROVIDER_LIMIT': return tr('provider_vault_code_limit');
    case 'RATE_LIMITED': return tr('provider_vault_code_rate');
    case 'ID_CONFLICT': return tr('provider_vault_code_conflict');
    case 'SIGN_IN_REQUIRED': case 'INVALID_SESSION': case 'REAUTH_REQUIRED': return tr('provider_vault_code_session');
    case 'INSTALL_FAILED': return tr('provider_vault_code_install');
    case 'PROVIDER_NOT_FOUND': return tr('provider_vault_code_missing');
    case 'REVISION_CONFLICT': return tr('provider_vault_code_changed');
    case 'INTERNAL': case 'INVALID_RESPONSE': case 'VAULT_UNREADABLE': return tr('provider_vault_code_server');
    default: return code && /^HTTP_5\d\d$/u.test(code) ? tr('provider_vault_code_server') : tr('provider_vault_code_failed', code ?? 'INTERNAL');
  }
}

const bridgeGlobal = () => typeof window === 'undefined' ? undefined : window[DESKTOP_BRIDGE_GLOBAL];

/**
 * The providers in the signed-in account's cloud vault, with "pull to this Mac". A pull needs a sign-in within the last
 * few minutes, so a stale session asks for the account's credentials again (the same account only) and then continues.
 */
export function CloudProviders({ api = bridgeGlobal()?.providerVault, account = bridgeGlobal()?.account, onPulled }: {
  api?: ProviderVaultApi; account?: AccountApi; onPulled?: () => Promise<unknown>;
}) {
  useI18n();
  const t = useT();
  const { state } = useAccountState(account);
  const user = state?.user ?? null;
  const [providers, setProviders] = useState<VaultProvider[]>();
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [removal, setRemoval] = useState<VaultProvider>();
  const [reauth, setReauth] = useState<string[]>();
  const [password, setPassword] = useState('');
  const [reauthError, setReauthError] = useState<string>();
  const generation = useRef(0);
  const dialog = useRef<HTMLDialogElement>(null);

  const apply = useCallback((reply: ProviderVaultReply, current: number) => {
    if (generation.current !== current) return false;
    if (!reply.ok) { setError(tr('provider_vault_error', vaultCodeText(reply.code))); return false; }
    // A pull whose keys are in but whose list could not be read keeps the list shown.
    if (reply.providers) setProviders(reply.providers);
    setError(undefined);
    return true;
  }, []);
  useEffect(() => {
    const current = ++generation.current;
    setProviders(undefined); setError(undefined); setNotice(undefined);
    if (!api || !user) return;
    void api.command({ action: 'list' }).then(reply => apply(reply, current)).catch(() => { if (generation.current === current) setError(tr('provider_vault_error', vaultCodeText('NETWORK'))); });
  }, [api, user?.uid, apply]);
  useEffect(() => {
    if (reauth) { if (!dialog.current?.open) dialog.current?.showModal(); }
    else dialog.current?.close();
  }, [reauth]);

  const pull = async (ids: string[]) => {
    if (!api || busy) return;
    const current = generation.current;
    setBusy(ids[0]); setError(undefined); setNotice(undefined);
    try {
      const reply = await api.command({ action: 'pull', ids });
      if (generation.current !== current) return;
      if (!reply.ok && reply.code === 'REAUTH_REQUIRED') { setPassword(''); setReauthError(undefined); setReauth(ids); return; }
      if (!apply(reply, current) || !reply.ok) return;
      const failed = (reply.pulled ?? []).filter(item => !item.ok);
      const label = (id: string) => (reply.providers ?? providers)?.find(provider => provider.id === id)?.label ?? id;
      if (failed.length) setError(failed.map(item => tr('provider_vault_pull_failed', label(item.id), vaultCodeText(item.code))).join('\n'));
      if ((reply.pulled ?? []).some(item => item.ok)) { setNotice(tr('provider_vault_pulled')); await onPulled?.(); }
    } catch (cause) {
      if (generation.current === current) setError(cause instanceof Error ? cause.message : tr('provider_accounts_error'));
    } finally { if (generation.current === current) setBusy(undefined); }
  };
  const confirmIdentity = async (command: { type: 'signIn'; password: string } | { type: 'oauth'; provider: 'google.com' | 'apple.com' }) => {
    if (!account || !user || !reauth) return;
    const ids = reauth;
    setBusy('reauth'); setReauthError(undefined);
    try {
      const next = await account.request(command.type === 'signIn'
        ? { type: 'signIn', email: user.email ?? '', password: command.password, expectUid: user.uid }
        : { type: 'oauth', provider: command.provider, expectUid: user.uid });
      if (next.error || next.user?.uid !== user.uid) { setReauthError(next.error ?? tr('provider_vault_code_session')); return; }
    } catch (cause) { setReauthError(cause instanceof Error ? cause.message : tr('provider_accounts_error')); return; }
    finally { setPassword(''); setBusy(undefined); }
    setReauth(undefined);
    await pull(ids);
  };
  /** Closing the dialog also ends a browser sign-in it started, which would otherwise hold the account for minutes. */
  const closeReauth = () => {
    if (busy === 'reauth') void account?.request({ type: 'cancel' }).catch(() => undefined);
    setReauth(undefined);
  };
  const remove = async (provider: VaultProvider) => {
    if (!api || busy) return;
    const current = generation.current;
    setBusy(provider.id); setError(undefined); setNotice(undefined);
    try { if (apply(await api.command({ action: 'remove', id: provider.id, expectedRevision: provider.revision }), current)) setRemoval(undefined); }
    catch (cause) { if (generation.current === current) setError(cause instanceof Error ? cause.message : tr('provider_accounts_error')); }
    finally { if (generation.current === current) setBusy(undefined); }
  };

  if (!api || !account || !state) return null;
  // Only the ways this account signs in; all of them while they are not known yet.
  const methods = user?.signInMethods, offers = (method: 'password' | 'google.com' | 'apple.com') => !methods || methods.length === 0 || methods.includes(method);
  const passwordOffered = Boolean(user?.email) && offers('password');
  const theme = { '--pa-bg': t.surface, '--pa-text': t.text, '--pa-secondary': t.text3, '--pa-border': t.border, '--pa-accent': t.accent, '--pa-ok': t.ok, '--pa-danger': t.danger } as CSSProperties;
  return <section className="pa-panel cloud-providers" style={theme} aria-label={tr('provider_vault_title')} aria-busy={Boolean(busy)} data-testid="cloud-providers">
    <header className="pa-section-heading"><div><h3>{tr('provider_vault_title')} {providers && <span className="pa-count">{providers.length}</span>}</h3><p>{user ? tr('provider_vault_desc') : tr('provider_vault_sign_in')}</p></div></header>
    {user && <div className="pa-list">
      {!providers ? !error && <div className="pa-empty" role="status"><span className="pa-spinner" />{tr('provider_accounts_loading')}</div>
        : providers.length === 0 ? <p className="pa-caption">{tr('provider_vault_empty')}</p>
        : providers.map(provider => <article className="pa-account" key={provider.id} data-testid={`cloud-provider-${provider.id}`}>
          <span className="pa-account-icon"><Icon name="key" size={19} /></span>
          <div className="pa-account-copy">
            <strong className="pa-name">{provider.label}</strong>
            <div className="pa-meta"><span>{tr(`provider_vault_source_${provider.source}`)}</span><span>•••• {provider.keyHint}</span>{!provider.webUsable && <span>{tr('provider_vault_desktop_only')}</span>}</div>
          </div>
          <div className="pa-account-actions">
            <button className="pa-button" disabled={Boolean(busy)} onClick={() => void pull([provider.id])}>{busy === provider.id && <span className="pa-spinner" />}{tr(busy === provider.id ? 'provider_vault_pulling' : 'provider_vault_pull')}</button>
            {removal?.id === provider.id
              ? <><button className="pa-button" disabled={Boolean(busy)} onClick={() => setRemoval(undefined)}>{tr('provider_accounts_cancel')}</button><button className="pa-button pa-danger-button" disabled={Boolean(busy)} onClick={() => void remove(provider)}>{tr('provider_vault_remove')}</button></>
              : <button className="pa-icon-button" aria-label={`${tr('provider_vault_remove')} ${provider.label}`} disabled={Boolean(busy)} onClick={() => { setError(undefined); setRemoval(provider); }}><Icon name="trash" size={15} /></button>}
          </div>
          {removal?.id === provider.id && <p className="pa-caption" role="alert">{tr('provider_vault_remove_confirm', provider.label)}</p>}
        </article>)}
    </div>}
    {error && <p className="pa-feedback pa-error" role="alert" style={{ whiteSpace: 'pre-line' }}>{error}</p>}
    {notice && <p className="pa-feedback pa-success" role="status"><Icon name="check" size={15} />{notice}</p>}
    <dialog ref={dialog} className="pa-dialog" aria-labelledby="cloud-reauth-title" onCancel={event => { event.preventDefault(); closeReauth(); }}>
      {reauth && <form onSubmit={event => { event.preventDefault(); if (passwordOffered && password && busy !== 'reauth') void confirmIdentity({ type: 'signIn', password }); }}>
        <header><h3 id="cloud-reauth-title">{tr('provider_vault_reauth_title')}</h3></header>
        <p className="pa-dialog-note">{tr('provider_vault_reauth_desc', user?.email ?? user?.displayName ?? '')}</p>
        {passwordOffered && <label className="pa-field">{tr('provider_vault_reauth_password')}<input type="password" autoComplete="current-password" value={password} onChange={event => setPassword(event.target.value)} disabled={busy === 'reauth'} /></label>}
        <div className="pa-reauth-oauth">
          {offers('google.com') && <button type="button" className="pa-button" disabled={busy === 'reauth'} onClick={() => void confirmIdentity({ type: 'oauth', provider: 'google.com' })}>{tr('provider_vault_reauth_google')}</button>}
          {offers('apple.com') && <button type="button" className="pa-button" disabled={busy === 'reauth'} onClick={() => void confirmIdentity({ type: 'oauth', provider: 'apple.com' })}>{tr('provider_vault_reauth_apple')}</button>}
        </div>
        {busy === 'reauth' && <p className="pa-progress" role="status"><span className="pa-spinner" />{tr('provider_vault_pulling')}</p>}
        {reauthError && <p className="pa-feedback pa-error" role="alert">{reauthError}</p>}
        <footer>
          <button type="button" className="pa-button" onClick={closeReauth}>{tr('provider_accounts_cancel')}</button>
          {passwordOffered && <button type="submit" className="pa-button pa-primary" disabled={busy === 'reauth' || !password}>{tr('provider_vault_reauth_submit')}</button>}
        </footer>
      </form>}
    </dialog>
  </section>;
}
