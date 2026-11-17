import { useState } from 'react';
import { GoogleAuthProvider, createUserWithEmailAndPassword, signInWithEmailAndPassword, signInWithPopup, type Auth } from 'firebase/auth';
import { useTeamsText } from '../../../../packages/teams/i18n';

/** The signed-out state of the Teams and Friends pages: Google, or an email and a password. `note` says what signing in is for. */
export function SignInCard({ auth, locale, note }: { auth: Auth; locale: string; note: string }) {
  const t = useTeamsText(locale);
  const [email, setEmail] = useState(''), [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [register, setRegister] = useState(false);
  const attempt = async (action: () => Promise<unknown>) => {
    setBusy(true); setError('');
    try { await action(); } catch { setError(t('tw_signin_error')); }
    finally { setBusy(false); }
  };
  return <section className="login-card teams-login"><h2>{register ? t('tw_signin_title_register') : t('tw_signin_title')}</h2><p>{note}</p><button type="button" className="button secondary" disabled={busy} onClick={() => void attempt(() => signInWithPopup(auth, new GoogleAuthProvider()))}>{t('account_google')}</button>
    <form className="login-form" onSubmit={event => { event.preventDefault(); void attempt(() => register ? createUserWithEmailAndPassword(auth, email.trim(), password) : signInWithEmailAndPassword(auth, email.trim(), password)); }}><label>{t('account_email')}<input type="email" autoComplete="email" value={email} onChange={event => setEmail(event.target.value)} required /></label><label>{t('account_password')}<input type="password" autoComplete={register ? 'new-password' : 'current-password'} minLength={register ? 6 : undefined} value={password} onChange={event => setPassword(event.target.value)} required /></label><button className="button primary" disabled={busy}>{register ? t('tw_signin_register_action') : t('remote_sign_in')}</button></form><button type="button" disabled={busy} onClick={() => { setRegister(value => !value); setError(''); }}>{register ? t('account_existing') : t('tw_signin_new_account')}</button>{error ? <p role="alert">{error}</p> : null}</section>;
}
