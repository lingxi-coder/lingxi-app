import { tr } from '../../../i18n/locale';
import { useI18n } from '../../../i18n/useI18n';
import { useEffect, useRef, useState, type CSSProperties } from 'react';
import type { ProviderAccountCommand, ProviderAccountHealth, ProviderAccountRouting, ProviderAccountsApi, ProviderAccountSnapshot } from '../../../../shared/providerAccounts';
import { logicalProviderId, orderedProviderAccounts } from '../../../../shared/providerAccounts';
import type { ProviderDefinition } from '../../../../shared/providers';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';

export function providerAccountHealthLabel(health?: ProviderAccountHealth): string {
  if (!health) return tr('provider_accounts_health_unknown');
  const recovery = health.cooldownUntilMs ? ` · ${tr('provider_accounts_retry_after', new Date(health.cooldownUntilMs).toLocaleTimeString())}` : '';
  return `${tr(`provider_accounts_health_${health.status}`)}${recovery}`;
}

/** User priority stays stable; observed availability is shown independently. */
export function ProviderAccounts({ api, provider, running, onConfigured }: {
  api: ProviderAccountsApi; provider: ProviderDefinition; running: boolean; onConfigured?: () => Promise<unknown>;
}) {
  useI18n();
  const t = useT();
  const [routing, setRouting] = useState<ProviderAccountRouting>();
  const [snapshot, setSnapshot] = useState<ProviderAccountSnapshot>();
  const [name, setName] = useState('');
  const [key, setKey] = useState('');
  const [replacementId, setReplacementId] = useState<string>();
  const [formOpen, setFormOpen] = useState(false);
  const [authKind, setAuthKind] = useState<'api_key' | 'oauth'>('api_key');
  const [removalId, setRemovalId] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [oauthBusy, setOauthBusy] = useState(false);
  const [pending, setPending] = useState<{ action: string; accountId?: string }>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const generation = useRef(0);
  const busyRef = useRef(false);
  const activeOperation = useRef<number>();
  const form = useRef<HTMLDialogElement>(null);
  const confirmation = useRef<HTMLDialogElement>(null);
  const addButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const current = ++generation.current;
    const refresh = () => { if (!busyRef.current) void api.snapshot().then(value => { if (generation.current === current) setSnapshot(value); })
      .catch(() => { if (generation.current === current) setError(tr('provider_accounts_status_error')); }); };
    refresh();
    void api.routing?.().then(value => { if (generation.current === current) setRouting(value); }).catch(() => undefined);
    const unsubscribe = api.onRouting?.(event => { if (generation.current === current) setRouting(event); });
    const timer = setInterval(refresh, 15_000);
    return () => { ++generation.current; clearInterval(timer); unsubscribe?.(); if (activeOperation.current !== undefined) void api.cancel(activeOperation.current).catch(() => undefined); };
  }, [api, provider.id]);
  useEffect(() => {
    if (formOpen) {
      if (!form.current?.open) {
        form.current?.showModal();
        form.current?.querySelector<HTMLInputElement>('input:not(:disabled)')?.focus();
      }
    }
    else if (form.current?.open) { form.current.close(); addButton.current?.focus(); }
  }, [formOpen]);
  useEffect(() => {
    if (removalId) { if (!confirmation.current?.open) confirmation.current?.showModal(); }
    else confirmation.current?.close();
  }, [removalId]);
  const accounts = snapshot ? orderedProviderAccounts(snapshot, provider.id) : [];
  const locked = busy || running || !snapshot;
  const supportsKey = provider.authMethods.includes('api_key') || provider.authMethods.includes('token');
  const supportsOAuth = provider.authMethods.includes('oauth');
  const perform = async (command: Omit<ProviderAccountCommand, 'expectedRevision'>, credential?: string) => {
    if (!snapshot || busyRef.current) return;
    const current = generation.current;
    activeOperation.current = Math.floor(Math.random() * Number.MAX_SAFE_INTEGER);
    busyRef.current = true; setBusy(true); setPending({ action: command.action, accountId: command.accountId });
    setOauthBusy(command.authKind === 'oauth' || (command.action === 'reconnect' && accounts.find(a => a.accountId === command.accountId)?.authKind === 'oauth'));
    setError(undefined); setNotice(undefined);
    try {
      const next = await api.command({ ...command, operationId: activeOperation.current, expectedRevision: snapshot.configRevision }, credential);
      if (generation.current !== current) return;
      setSnapshot(next);
      if (command.action === 'add' || command.action === 'reconnect') {
        setKey(''); setName(''); setReplacementId(undefined); setFormOpen(false);
        setNotice(tr('provider_accounts_connected'));
        await onConfigured?.();
      }
      if (command.action === 'remove') setRemovalId(undefined);
      if (command.action === 'test') setNotice(tr('provider_accounts_test_success'));
    } catch (cause) {
      if (generation.current === current) {
        setError(cause instanceof Error ? cause.message : tr('provider_accounts_error'));
        const latest = await api.snapshot().catch(() => undefined);
        if (latest && generation.current === current) setSnapshot(latest);
      }
    } finally {
      activeOperation.current = undefined; busyRef.current = false;
      if (generation.current === current) { setBusy(false); setOauthBusy(false); setPending(undefined); }
    }
  };
  const closeForm = () => {
    if (busy) return;
    setFormOpen(false); setKey(''); setName(''); setReplacementId(undefined); setError(undefined);
  };
  const openForm = (accountId?: string) => {
    const existing = accounts.find(account => account.accountId === accountId);
    setReplacementId(accountId); setName(existing?.displayName ?? ''); setKey(''); setError(undefined);
    setAuthKind(existing?.authKind ?? (supportsOAuth ? 'oauth' : 'api_key')); setFormOpen(true);
  };
  const move = (from: number, delta: number) => {
    const ids = accounts.map(a => a.accountId);
    [ids[from], ids[from + delta]] = [ids[from + delta], ids[from]];
    void perform({ action: 'reorder', providerId: logicalProviderId(provider.id), accountIds: ids });
  };
  const cancelLogin = () => { if (activeOperation.current !== undefined) void api.cancel(activeOperation.current).catch(() => setError(tr('provider_accounts_error'))); };
  const theme = { '--pa-bg': t.surface, '--pa-text': t.text, '--pa-secondary': t.text3, '--pa-border': t.border, '--pa-accent': t.accent, '--pa-ok': t.ok, '--pa-danger': t.danger } as CSSProperties;
  const menuAction = (button: HTMLButtonElement, action: () => void) => { button.closest('details')?.removeAttribute('open'); action(); };
  return <section className="pa-panel" style={theme} aria-label={tr('provider_accounts_accounts')} aria-busy={busy}>
    <header className="pa-section-heading">
      <div><h3>{tr('provider_accounts_accounts')} {snapshot && <span className="pa-count">{accounts.length}</span>}</h3><p>{tr('provider_accounts_priority_desc')}</p></div>
      <div className="pa-toolbar">
        <button className="pa-icon-button" aria-label={tr('provider_accounts_refresh')} disabled={busy} onClick={() => void api.snapshot().then(setSnapshot).catch(() => setError(tr('provider_accounts_status_error')))}><Icon name="refresh" size={16} /></button>
        {provider.available && <button ref={addButton} className="pa-button pa-primary" disabled={locked} onClick={() => openForm()}><Icon name="plus" size={15} />{tr('provider_accounts_add')}</button>}
      </div>
    </header>
    <div className="pa-list">
      {!snapshot ? <div className="pa-empty" role="status"><span className="pa-spinner" />{tr('provider_accounts_loading')}</div>
        : accounts.length === 0 ? <div className="pa-empty"><span className="pa-empty-icon"><Icon name="user" size={24} /></span><h4>{tr('provider_accounts_empty')}</h4><p>{tr('provider_accounts_empty_desc')}</p></div>
        : accounts.map((account, index) => {
          const health = snapshot.health.find(h => h.accountId === account.accountId && h.identityGeneration === account.identityGeneration);
          const entitlements = snapshot.modelEntitlements?.filter(entry => entry.accountId === account.accountId && entry.identityGeneration === account.identityGeneration) ?? [];
          const testing = pending?.action === 'test' && pending.accountId === account.accountId;
          return <article className="pa-account" key={account.accountId} data-enabled={account.enabled}>
            <span className="pa-rank" aria-label={`${tr('provider_accounts_priority')} ${index + 1}`}>{index + 1}</span>
            <span className="pa-account-icon"><Icon name={account.authKind === 'oauth' ? 'user' : 'key'} size={19} /></span>
            <div className="pa-account-copy">
              <input className="pa-name" aria-label={tr('provider_accounts_row_name', index + 1)} key={`${account.accountId}:${account.configRevision}`} defaultValue={account.displayName} disabled={locked} maxLength={128}
                onKeyDown={event => { if (event.key === 'Enter') event.currentTarget.blur(); if (event.key === 'Escape') { event.currentTarget.value = account.displayName; event.currentTarget.blur(); event.stopPropagation(); } }}
                onBlur={event => { const displayName = event.currentTarget.value.trim(); if (!displayName) event.currentTarget.value = account.displayName; else if (displayName !== account.displayName) void perform({ action: 'update', accountId: account.accountId, displayName }); }} />
              <div className="pa-meta"><span>{account.authKind === 'oauth' ? tr('provider_accounts_subscription') : tr('provider_accounts_key')}</span><span className="pa-health" data-status={health?.status} role="status"><i />{providerAccountHealthLabel(health)}</span></div>
              {routing?.selectedAccountId === account.accountId && routing.identityGeneration === account.identityGeneration && <p className="pa-caption">{tr('provider_accounts_last_used', routing.model, routing.attempt)}</p>}
              {entitlements.length ? <details className="pa-permissions"><summary>{tr('provider_accounts_model_access')}</summary><ul>{entitlements.map(entry => <li key={entry.modelId}><span>{entry.modelId}</span><span>{tr(`provider_accounts_model_${entry.status}`)}</span></li>)}</ul></details> : <p className="pa-caption">{tr('provider_accounts_models_unverified')}</p>}
            </div>
            <div className="pa-account-actions">
              <div className="pa-reorder"><button className="pa-icon-button" aria-label={tr('provider_accounts_move_up', account.displayName)} disabled={locked || index === 0} onClick={() => move(index, -1)}>↑</button><button className="pa-icon-button" aria-label={tr('provider_accounts_move_down', account.displayName)} disabled={locked || index === accounts.length - 1} onClick={() => move(index, 1)}>↓</button></div>
              <button className="pa-button pa-test" data-testid={`provider-account-test-${account.accountId}`} disabled={locked} onClick={() => void perform({ action: 'test', accountId: account.accountId })}>{testing && <span className="pa-spinner" />}{tr(testing ? 'provider_accounts_testing' : 'provider_accounts_test')}</button>
              <label className="pa-switch"><input type="checkbox" role="switch" aria-label={`${tr(account.enabled ? 'provider_accounts_disable' : 'provider_accounts_enable')} ${account.displayName}`} checked={account.enabled} disabled={locked} onChange={() => void perform({ action: 'enable', accountId: account.accountId, enabled: !account.enabled })} /><span /></label>
              <details className="pa-more" onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) event.currentTarget.open = false; }} onKeyDown={event => { if (event.key === 'Escape') { event.currentTarget.open = false; event.stopPropagation(); } }}>
                <summary className="pa-icon-button" aria-label={tr('provider_accounts_more', account.displayName)}><Icon name="more" /></summary>
                <div className="pa-menu"><button disabled={locked} onClick={event => menuAction(event.currentTarget, () => openForm(account.accountId))}>{tr('provider_accounts_reconnect')}</button><button className="pa-destructive" disabled={locked} onClick={event => menuAction(event.currentTarget, () => { setError(undefined); setRemovalId(account.accountId); })}>{tr('provider_accounts_remove')}</button></div>
              </details>
            </div>
          </article>;
        })}
    </div>
    {running && <p className="pa-caption">{tr('provider_accounts_running')}</p>}
    {logicalProviderId(provider.id) === 'openai' && snapshot && <div className="pa-billing"><div><h4>{tr('provider_accounts_billing')}</h4><p>{tr('provider_accounts_billing_desc')}</p></div><label className="pa-switch"><input type="checkbox" role="switch" aria-label={tr('provider_accounts_billing_allow')} checked={snapshot.providerPolicies?.openai?.allowSubscriptionToMetered === true} disabled={locked} onChange={event => void perform({ action: 'set_policy', providerId: 'openai', allowSubscriptionToMetered: event.currentTarget.checked })} /><span /></label></div>}
    {error && !formOpen && !removalId && <p className="pa-feedback pa-error" role="alert">{error}</p>}
    {notice && <p className="pa-feedback pa-success" role="status"><Icon name="check" size={15} />{notice}</p>}
    <dialog ref={form} className="pa-dialog" aria-labelledby="pa-form-title" onCancel={event => { event.preventDefault(); if (busy && oauthBusy) cancelLogin(); else closeForm(); }} onKeyDown={event => { if (event.key === 'Escape') event.stopPropagation(); }}>
      <form onSubmit={event => { event.preventDefault(); if (!locked && (authKind === 'oauth' || key.trim())) void perform({ action: replacementId ? 'reconnect' : 'add', accountId: replacementId, providerId: logicalProviderId(provider.id), displayName: name.trim(), authKind, executionBindingId: authKind === 'oauth' ? 'openai-chatgpt' : logicalProviderId(provider.id) }, authKind === 'api_key' ? key : undefined); }}>
        <header><span className="pa-dialog-icon"><Icon name={authKind === 'oauth' ? 'user' : 'key'} size={24} /></span><h3 id="pa-form-title">{tr(replacementId ? authKind === 'oauth' ? 'provider_accounts_reconnect' : 'provider_accounts_replace' : 'provider_accounts_add')}</h3><p>{provider.label}</p></header>
        {supportsKey && supportsOAuth && !replacementId && <div className="pa-segment" aria-label={tr('provider_accounts_method')}><button type="button" aria-pressed={authKind === 'oauth'} disabled={busy} onClick={() => setAuthKind('oauth')}>ChatGPT</button><button type="button" aria-pressed={authKind === 'api_key'} disabled={busy} onClick={() => setAuthKind('api_key')}>{tr('provider_accounts_key')}</button></div>}
        <label className="pa-field">{tr('provider_accounts_name')}<input autoFocus autoComplete="off" value={name} onChange={event => setName(event.target.value)} disabled={busy || Boolean(replacementId)} maxLength={128} placeholder={provider.label} /></label>
        {authKind === 'api_key' && <label className="pa-field">{tr('provider_accounts_key')}<input type="password" autoComplete="new-password" value={key} onChange={event => setKey(event.target.value)} disabled={busy} required /></label>}
        <p className="pa-dialog-note">{tr(authKind === 'oauth' ? 'provider_accounts_browser_hint' : 'provider_accounts_storage')}</p>
        {busy && <p className="pa-progress" role="status"><span className="pa-spinner" />{tr(oauthBusy ? 'provider_accounts_browser_wait' : 'provider_accounts_saving')}</p>}
        {error && <p className="pa-feedback pa-error" role="alert">{error}</p>}
        <footer><button type="button" className="pa-button" disabled={busy && !oauthBusy} onClick={() => busy && oauthBusy ? cancelLogin() : closeForm()}>{tr(busy && oauthBusy ? 'provider_accounts_cancel_login' : 'provider_accounts_cancel')}</button><button type="submit" className="pa-button pa-primary" disabled={locked || (authKind === 'api_key' && !key.trim())}>{tr(replacementId ? authKind === 'oauth' ? 'provider_accounts_reconnect' : 'provider_accounts_save' : authKind === 'oauth' ? 'provider_accounts_add_oauth' : 'provider_accounts_add_key')}</button></footer>
      </form>
    </dialog>
    <dialog ref={confirmation} className="pa-dialog" aria-labelledby="pa-remove-title" onCancel={event => { event.preventDefault(); if (!busy) setRemovalId(undefined); }} onKeyDown={event => { if (event.key === 'Escape') event.stopPropagation(); }}><h3 id="pa-remove-title">{tr('provider_accounts_remove')} {accounts.find(account => account.accountId === removalId)?.displayName}?</h3><p className="pa-dialog-note">{tr('provider_accounts_remove_hint')}</p>{error && <p className="pa-feedback pa-error" role="alert">{error}</p>}<footer><button className="pa-button" disabled={busy} onClick={() => setRemovalId(undefined)}>{tr('provider_accounts_cancel')}</button><button className="pa-button pa-danger-button" disabled={locked} onClick={() => removalId && void perform({ action: 'remove', accountId: removalId })}>{tr('provider_accounts_remove')}</button></footer></dialog>
  </section>;
}
