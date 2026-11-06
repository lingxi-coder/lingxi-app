import { useEffect, useState } from 'react';
import { GoogleAuthProvider, createUserWithEmailAndPassword, onAuthStateChanged, signInWithEmailAndPassword, signInWithPopup, signOut, type User } from 'firebase/auth';
import { teamsDatabase } from '../teams/firestore';
import { auth } from '../app/firebase';
import { webTeamsApi } from '../teams/api';
import { TeamsWorkspaceView } from '../teams/TeamsWorkspaceView';
import { clearInviteIntent, readInviteIntent, retainInviteIntent } from '../teams/invite-intent';
import type { Locale } from '../utils/locale';

const inviteStorage = {
  getItem(key: string) { return sessionStorage.getItem(key); },
  setItem(key: string, value: string) { sessionStorage.setItem(key, value); },
  removeItem(key: string) { sessionStorage.removeItem(key); },
};

function SignedIn({ user, zh, invite, onInviteHandled, onInviteResolved }: { user: User; zh: boolean; invite: string; onInviteHandled(): void; onInviteResolved(): void }) {
  const [api, setApi] = useState<ReturnType<typeof webTeamsApi>>();
  useEffect(() => { const current = webTeamsApi(user, teamsDatabase(auth.app)); setApi(current); return () => current.close(); }, [user]);
  return api ? <TeamsWorkspaceView api={api} uid={user.uid} zh={zh} invite={invite} onInviteHandled={onInviteHandled} onInviteResolved={onInviteResolved} /> : null;
}
export default function TeamsPage({ locale }: { locale: Locale }) {
  const zh = locale === 'zh';
  const [user, setUser] = useState<User | null | undefined>(undefined);
  const [code, setCode] = useState(() => readInviteIntent(`https://lingxios.net${location.pathname}${location.search}${location.hash}`, inviteStorage));
  const [email, setEmail] = useState(''), [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [register, setRegister] = useState(false);
  useEffect(() => {
    // Strip the link only after the continuation intent is retained. A browser
    // denying storage can still recover it from the URL after a refresh.
    let retained = !code;
    if (code) retained = retainInviteIntent(code, inviteStorage);
    if (retained && (location.search || location.hash)) history.replaceState(null, '', location.pathname);
    return onAuthStateChanged(auth, setUser);
  }, []);
  const attempt = async (action: () => Promise<unknown>) => {
    setBusy(true); setError('');
    try { await action(); } catch { setError(zh ? '账号操作未完成，请检查邮箱、密码和网络后重试。' : 'Could not complete the account request. Check your email, password and connection, then try again.'); }
    finally { setBusy(false); }
  };
  return <main className="teams-web"><header className="teams-web-nav"><a href="/">LingXi</a><h1>{zh ? '团队' : 'Teams'}</h1><a href="/remote">{zh ? '远控' : 'Remote'}</a><span>{user?.displayName || user?.email}</span>{user ? <button disabled={busy} onClick={() => void attempt(() => signOut(auth))}>{zh ? '退出登录' : 'Sign out'}</button> : null}</header>
    {user === undefined ? <p role="status" className="teams-web-note">{zh ? '正在加载…' : 'Loading…'}</p> : user ? <SignedIn key={user.uid} user={user} zh={zh} invite={code} onInviteHandled={() => setCode('')} onInviteResolved={() => { clearInviteIntent(inviteStorage); setCode(''); history.replaceState(null, '', location.pathname); }} /> : <section className="login-card teams-login"><h2>{register ? (zh ? '注册灵犀账号' : 'Create your LingXi account') : (zh ? '登录灵犀团队' : 'Sign in to LingXi Teams')}</h2><p>{code ? (zh ? '登录后确认加入团队。' : 'Sign in to review and confirm the invitation.') : (zh ? '使用灵犀账号创建或加入团队。' : 'Create or join a team with your LingXi account.')}</p><button type="button" className="button secondary" disabled={busy} onClick={() => void attempt(() => signInWithPopup(auth, new GoogleAuthProvider()))}>{zh ? '使用 Google 登录' : 'Continue with Google'}</button>
      <form className="login-form" onSubmit={event => { event.preventDefault(); void attempt(() => register ? createUserWithEmailAndPassword(auth, email.trim(), password) : signInWithEmailAndPassword(auth, email.trim(), password)); }}><label>{zh ? '邮箱' : 'Email'}<input type="email" autoComplete="email" value={email} onChange={event => setEmail(event.target.value)} required /></label><label>{zh ? '密码' : 'Password'}<input type="password" autoComplete={register ? 'new-password' : 'current-password'} minLength={register ? 6 : undefined} value={password} onChange={event => setPassword(event.target.value)} required /></label><button className="button primary" disabled={busy}>{register ? (zh ? '注册并继续' : 'Create account and continue') : (zh ? '登录' : 'Sign in')}</button></form><button type="button" disabled={busy} onClick={() => { setRegister(value => !value); setError(''); }}>{register ? (zh ? '已有账号？登录' : 'Already have an account? Sign in') : (zh ? '没有账号？注册' : 'New to LingXi? Create an account')}</button>{error ? <p role="alert">{error}</p> : null}</section>}
  </main>;
}
