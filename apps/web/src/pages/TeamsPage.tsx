import { useEffect, useState } from 'react';
import { GoogleAuthProvider, onAuthStateChanged, signInWithEmailAndPassword, signInWithPopup, signOut, type User } from 'firebase/auth';
import { teamsDatabase } from '../teams/firestore';
import { auth } from '../app/firebase';
import { webTeamsApi } from '../teams/api';
import { TeamsWorkspaceView } from '../teams/TeamsWorkspaceView';
import { inviteCode } from '../../../../packages/teams';
import type { Locale } from '../utils/locale';

function SignedIn({ user, zh, invite, onInviteHandled }: { user: User; zh: boolean; invite: string; onInviteHandled(): void }) {
  const [api, setApi] = useState<ReturnType<typeof webTeamsApi>>();
  useEffect(() => { const current = webTeamsApi(user, teamsDatabase(auth.app)); setApi(current); return () => current.close(); }, [user]);
  return api ? <TeamsWorkspaceView api={api} uid={user.uid} zh={zh} invite={invite} onInviteHandled={onInviteHandled} /> : null;
}
export default function TeamsPage({ locale }: { locale: Locale }) {
  const zh = locale === 'zh';
  const [user, setUser] = useState<User | null | undefined>(undefined);
  const [code, setCode] = useState(() => inviteCode(`https://lingxios.net${location.pathname}${location.search}${location.hash}`) ?? '');
  const [email, setEmail] = useState(''), [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  useEffect(() => {
    // The invitation was read above; it does not stay in the address bar or the history.
    if (location.search || location.hash) history.replaceState(null, '', location.pathname);
    return onAuthStateChanged(auth, setUser);
  }, []);
  const attempt = async (action: () => Promise<unknown>) => {
    setBusy(true); setError('');
    try { await action(); } catch { setError(zh ? '登录未完成，请检查账号和网络后重试。' : 'Sign-in failed. Check your account and connection, then try again.'); }
    finally { setBusy(false); }
  };
  return <main className="teams-web"><header className="teams-web-nav"><a href="/">LingXi</a><h1>{zh ? '团队' : 'Teams'}</h1><a href="/remote">{zh ? '远控' : 'Remote'}</a><span>{user?.displayName || user?.email}</span>{user ? <button disabled={busy} onClick={() => void attempt(() => signOut(auth))}>{zh ? '退出登录' : 'Sign out'}</button> : null}</header>
    {user === undefined ? <p role="status" className="teams-web-note">{zh ? '正在加载…' : 'Loading…'}</p> : user ? <SignedIn key={user.uid} user={user} zh={zh} invite={code} onInviteHandled={() => setCode('')} /> : <section className="login-card teams-login"><h2>{zh ? '登录灵犀团队' : 'Sign in to LingXi Teams'}</h2><p>{code ? (zh ? '登录后确认加入团队。' : 'Sign in to review and confirm the invitation.') : (zh ? '使用灵犀账号创建或加入团队。' : 'Create or join a team with your LingXi account.')}</p><button type="button" className="button secondary" disabled={busy} onClick={() => void attempt(() => signInWithPopup(auth, new GoogleAuthProvider()))}>{zh ? '使用 Google 登录' : 'Continue with Google'}</button>
      <form className="login-form" onSubmit={event => { event.preventDefault(); void attempt(() => signInWithEmailAndPassword(auth, email, password)); }}><label>{zh ? '邮箱' : 'Email'}<input type="email" autoComplete="email" value={email} onChange={event => setEmail(event.target.value)} required /></label><label>{zh ? '密码' : 'Password'}<input type="password" autoComplete="current-password" value={password} onChange={event => setPassword(event.target.value)} required /></label><button className="button primary" disabled={busy}>{zh ? '登录' : 'Sign in'}</button></form><small>{zh ? '也可以先在灵犀应用内注册账号。' : 'You can register in the LingXi app.'}</small>{error ? <p role="alert">{error}</p> : null}</section>}
  </main>;
}
