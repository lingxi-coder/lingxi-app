import { tr } from '../../../i18n/locale';
import { useEffect, useRef, useState } from 'react';
import type { AccountCommand, AccountState } from '../../../../shared/account';
import { useT } from '../../../theme/ThemeContext';
import { ghostButtonStyle } from './ghostButton';

const googleIcon = new URL('../../../assets/google-g.png', import.meta.url).href;

export function LingxiAccount() {
  const t = useT();
  const [state, setState] = useState<AccountState | null>(null);
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [register, setRegister] = useState(false);
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState('');
  const inFlight = useRef(false);
  useEffect(() => {
    const api = window.lingxi?.account;
    if (!api) return;
    let active = true;
    let revision = 0;
    const off = api.onChanged(value => { revision++; if (active) setState(value); });
    void api.request({ type: 'status' }).then(value => {
      if (active && revision === 0) setState(value);
    }).catch(() => { if (active) setMessage(tr('desktop_shell_account_read_error')); });
    return () => { active = false; off(); };
  }, []);
  const busy = pending || Boolean(state?.busy);
  const run = async (command: AccountCommand) => {
    const api = window.lingxi?.account;
    if (!api || (inFlight.current && command.type !== 'cancel')) return;
    if (command.type === 'cancel') { await api.request(command).catch(() => undefined); return; }
    inFlight.current = true; setPending(true); setMessage('');
    try {
      const next = await api.request(command);
      setState(next);
      if (next.user || command.type === 'signOut') setPassword('');
      if (!next.error) {
        if (['signIn', 'register', 'oauth', 'signOut'].includes(command.type)) setPassword('');
        if (command.type === 'resetPassword') setMessage(tr('account_reset_sent'));
        if (command.type === 'verifyEmail') setMessage(tr('account_verification_sent'));
      }
    } catch (error) { setMessage(error instanceof Error ? error.message : tr('desktop_shell_operation_incomplete')); }
    finally { inFlight.current = false; setPending(false); }
  };
  const button = ghostButtonStyle(t, busy);
  const inputStyle = { background: t.surface, color: t.text, border: `1px solid ${t.border}`, borderRadius: 10, padding: '12px 14px', width: '100%', boxSizing: 'border-box' as const, font: 'inherit', minHeight: 46 };
  const primaryButton = { border: 0, borderRadius: 10, minHeight: 46, padding: '11px 16px', font: 'inherit', fontWeight: 550, cursor: busy ? 'default' : 'pointer' as const, opacity: busy ? .65 : 1 };
  return <div style={{ marginBottom: 28 }}>
    <div style={{ padding: '34px 24px 28px', display: 'grid', justifyItems: 'center', gap: 22 }}>
      <h2 style={{ margin: '0 0 4px', color: t.text, fontFamily: 'Georgia, "Times New Roman", serif', fontSize: 30, lineHeight: 1.15, letterSpacing: '-.035em', fontWeight: 600 }}>{tr('desktop_shell_sign_in')}</h2>
      <div style={{ width: 'min(100%, 460px)', padding: '22px 24px 18px', display: 'grid', gap: 14, border: `1px solid ${t.border}`, borderRadius: 22, background: t.surface, boxShadow: '0 12px 40px rgba(0,0,0,.055)' }}>
      {!state ? <p role="status">{tr('desktop_shell_account_loading')}</p> : state.user ? <>
        <strong>{state.user.displayName || state.user.email || tr('account_signed_in')}</strong>
        <span style={{ color: t.text3 }}>{state.user.email}</span>
        <span style={{ color: t.text3, fontSize: 12 }}>{state.user.emailVerified ? tr('account_email_verified') : tr('account_email_unverified')}</span>
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
          {!state.user.emailVerified && state.user.email && <button type="button" disabled={busy} style={button} onClick={() => void run({ type: 'verifyEmail' })}>{tr('account_send_verification')}</button>}
          <button type="button" disabled={busy} style={button} onClick={() => void run({ type: 'reload' })}>{tr('account_refresh')}</button>
          <button type="button" disabled={busy} style={button} onClick={() => void run({ type: 'signOut' })}>{tr('settings_logout')}</button>
        </div>
      </> : <>
        <button type="button" disabled={busy} style={{ ...primaryButton, width: '100%', display: 'inline-flex', alignItems: 'center', justifyContent: 'center', gap: 10, background: t.surfaceHover, color: t.text }} onClick={() => void run({ type: 'oauth', provider: 'google.com' })}><img src={googleIcon} alt="" width={20} height={20} />{tr('account_google')}</button>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12, color: t.text3, fontSize: 13 }}><span style={{ height: 1, background: t.border, flex: 1 }} />OR<span style={{ height: 1, background: t.border, flex: 1 }} /></div>
        <form onSubmit={event => { event.preventDefault(); void run({ type: register ? 'register' : 'signIn', email, password }); }} style={{ display: 'grid', gap: 12 }}>
          <input aria-label={tr('account_email')} placeholder={tr('account_email')} type="email" autoComplete="username" required value={email} onChange={e => setEmail(e.target.value)} disabled={busy} style={inputStyle} />
          <input aria-label={tr('account_password')} placeholder={tr('account_password')} type="password" autoComplete={register ? 'new-password' : 'current-password'} minLength={register ? 6 : undefined} required value={password} onChange={e => setPassword(e.target.value)} disabled={busy} style={inputStyle} />
          <button type="submit" disabled={busy} style={{ ...primaryButton, width: '100%', background: t.text, color: t.surface }}>{register ? tr('account_create') : tr('settings_provider_login')}</button>
        </form>
        <div style={{ textAlign: 'center', color: t.text3, fontSize: 12.5, lineHeight: 1.6 }}>{tr('desktop_shell_browser_login_desc')}</div>
      </>}
      <a href="https://lingxios.net/privacy" target="_blank" rel="noreferrer" style={{ color: t.text3, fontSize: 12 }}>{tr('desktop_shell_privacy_link')}</a>
      {busy && <div role="status">{tr('plugins_working')} <button type="button" style={ghostButtonStyle(t)} onClick={() => void run({ type: 'cancel' })}>{tr('desktop_shell_cancel_browser_login')}</button></div>}
      {(state?.error || message) && <p role={state?.error ? 'alert' : 'status'} style={{ margin: 0 }}>{state?.error || message}</p>}
      {state?.persistence === 'session' && <small style={{ color: t.text3 }}>{tr('desktop_shell_storage_unavailable')}</small>}
      </div>
      {!state?.user && <details style={{ color: t.text3, fontSize: 13 }}><summary style={{ cursor: 'pointer', textAlign: 'center', textDecoration: 'underline', textUnderlineOffset: 3 }}>{tr('desktop_shell_account_other_signin')}</summary>
        <div style={{ display: 'flex', justifyContent: 'center', flexWrap: 'wrap', gap: 8, paddingTop: 12 }}>
          <button type="button" disabled={busy} style={button} onClick={() => void run({ type: 'oauth', provider: 'apple.com' })}>{tr('desktop_shell_apple_login')}</button>
          <button type="button" disabled={busy} style={button} onClick={() => { setRegister(!register); setPassword(''); setMessage(''); }}>{register ? tr('account_existing') : tr('account_register')}</button>
          <button type="button" disabled={busy || !email.trim()} style={button} onClick={() => void run({ type: 'resetPassword', email })}>{tr('account_forgot_password')}</button>
        </div>
      </details>}
    </div>
  </div>;
}
