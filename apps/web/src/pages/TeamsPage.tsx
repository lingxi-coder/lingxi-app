import { useEffect, useRef, useState } from 'react';
import { onAuthStateChanged, signOut, type User } from 'firebase/auth';
import { teamsDatabase } from '../teams/firestore';
import { auth } from '../app/firebase';
import { webFriendsApi } from '../friends/api';
import { webTeamsApi } from '../teams/api';
import { SignInCard } from '../teams/SignInCard';
import { TeamsWorkspaceView } from '../teams/TeamsWorkspaceView';
import { clearInviteIntent, readInviteIntent, retainInviteIntent } from '../teams/invite-intent';
import type { Locale } from '../utils/locale';

const inviteStorage = {
  getItem(key: string) { return localStorage.getItem(key); },
  setItem(key: string, value: string) { localStorage.setItem(key, value); },
  removeItem(key: string) { localStorage.removeItem(key); },
};

function SignedIn({ user, zh, invite, onInviteHandled, onInviteResolved }: { user: User; zh: boolean; invite: string; onInviteHandled(): void; onInviteResolved(): void }) {
  const [apis, setApis] = useState<{ teams: ReturnType<typeof webTeamsApi>; friends: ReturnType<typeof webFriendsApi> }>();
  useEffect(() => {
    const db = teamsDatabase(auth.app), current = { teams: webTeamsApi(user, db), friends: webFriendsApi(user, db) };
    setApis(current);
    return () => { current.teams.close(); current.friends.close(); };
  }, [user]);
  return apis ? <TeamsWorkspaceView api={apis.teams} friends={apis.friends} uid={user.uid} zh={zh} invite={invite} onInviteHandled={onInviteHandled} onInviteResolved={onInviteResolved} /> : null;
}
export default function TeamsPage({ locale }: { locale: Locale }) {
  const zh = locale === 'zh';
  const [user, setUser] = useState<User | null | undefined>(undefined);
  const [code, setCode] = useState(() => readInviteIntent(`https://lingxios.net${location.pathname}${location.search}${location.hash}`, inviteStorage));
  const handledInvite = useRef(code);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    // Strip the link only after the continuation intent is retained. A browser
    // denying storage can still recover it from the URL after a refresh.
    let retained = !code;
    if (code) retained = retainInviteIntent(code, inviteStorage);
    if (retained && (location.search || location.hash)) history.replaceState(null, '', location.pathname);
    return onAuthStateChanged(auth, setUser);
  }, []);
  const signOutNow = async () => {
    setBusy(true);
    try { await signOut(auth); } catch { /* The person stays signed in. */ }
    finally { setBusy(false); }
  };
  return <main className="teams-web"><header className="teams-web-nav"><a href="/">LingXi</a><a href="/friends">{zh ? '好友' : 'Friends'}</a><h1>{zh ? '团队' : 'Teams'}</h1><a href="/remote">{zh ? '远控' : 'Remote'}</a><span>{user?.displayName || user?.email}</span>{user ? <button disabled={busy} onClick={() => void signOutNow()}>{zh ? '退出登录' : 'Sign out'}</button> : null}</header>
    {user === undefined ? <p role="status" className="teams-web-note">{zh ? '正在加载…' : 'Loading…'}</p> : user ? <SignedIn key={user.uid} user={user} zh={zh} invite={code} onInviteHandled={() => setCode('')} onInviteResolved={() => { clearInviteIntent(inviteStorage, handledInvite.current); setCode(''); history.replaceState(null, '', location.pathname); }} /> : <SignInCard auth={auth} zh={zh} note={code ? (zh ? '登录后确认加入团队。' : 'Sign in to review and confirm the invitation.') : (zh ? '使用灵犀账号创建或加入团队。' : 'Create or join a team with your LingXi account.')} />}
  </main>;
}
