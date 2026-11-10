import { useState } from 'react';
import { GoogleAuthProvider, createUserWithEmailAndPassword, signInWithEmailAndPassword, signInWithPopup, type Auth } from 'firebase/auth';

/** The signed-out state of the Teams and Friends pages: Google, or an email and a password. `note` says what signing in is for. */
export function SignInCard({ auth, zh, note }: { auth: Auth; zh: boolean; note: string }) {
  const [email, setEmail] = useState(''), [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [register, setRegister] = useState(false);
  const attempt = async (action: () => Promise<unknown>) => {
    setBusy(true); setError('');
    try { await action(); } catch { setError(zh ? '账号操作未完成，请检查邮箱、密码和网络后重试。' : 'Could not complete the account request. Check your email, password and connection, then try again.'); }
    finally { setBusy(false); }
  };
  return <section className="login-card teams-login"><h2>{register ? (zh ? '注册灵犀账号' : 'Create your LingXi account') : (zh ? '登录灵犀团队' : 'Sign in to LingXi Teams')}</h2><p>{note}</p><button type="button" className="button secondary" disabled={busy} onClick={() => void attempt(() => signInWithPopup(auth, new GoogleAuthProvider()))}>{zh ? '使用 Google 登录' : 'Continue with Google'}</button>
    <form className="login-form" onSubmit={event => { event.preventDefault(); void attempt(() => register ? createUserWithEmailAndPassword(auth, email.trim(), password) : signInWithEmailAndPassword(auth, email.trim(), password)); }}><label>{zh ? '邮箱' : 'Email'}<input type="email" autoComplete="email" value={email} onChange={event => setEmail(event.target.value)} required /></label><label>{zh ? '密码' : 'Password'}<input type="password" autoComplete={register ? 'new-password' : 'current-password'} minLength={register ? 6 : undefined} value={password} onChange={event => setPassword(event.target.value)} required /></label><button className="button primary" disabled={busy}>{register ? (zh ? '注册并继续' : 'Create account and continue') : (zh ? '登录' : 'Sign in')}</button></form><button type="button" disabled={busy} onClick={() => { setRegister(value => !value); setError(''); }}>{register ? (zh ? '已有账号？登录' : 'Already have an account? Sign in') : (zh ? '没有账号？注册' : 'New to LingXi? Create an account')}</button>{error ? <p role="alert">{error}</p> : null}</section>;
}
