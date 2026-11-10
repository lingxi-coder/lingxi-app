import { useEffect, useRef, useState } from 'react';
import { onAuthStateChanged, signOut, type User } from 'firebase/auth';
import { auth } from '../app/firebase';
import { webFriendsApi } from '../friends/api';
import { clearAddIntent, readAddIntent, retainAddIntent } from '../friends/add-intent';
import { FriendsWorkspaceView } from '../friends/FriendsWorkspaceView';
import { SignInCard } from '../teams/SignInCard';
import { teamsDatabase } from '../teams/firestore';
import { webTeamsApi } from '../teams/api';
import type { Locale } from '../utils/locale';

const intentStorage = {
  getItem(key: string) { return localStorage.getItem(key); },
  setItem(key: string, value: string) { localStorage.setItem(key, value); },
  removeItem(key: string) { localStorage.removeItem(key); },
};

type Page = { route: '/friends' | '/friends/add'; badge: string | null; onBadge(text: string | null): void };

function SignedIn({ user, zh, route, chat, code, onCodeHandled, onBadge }: { user: User; zh: boolean; chat?: string; code: string; onCodeHandled(): void } & Pick<Page, 'route' | 'onBadge'>) {
  const [apis, setApis] = useState<{ friends: ReturnType<typeof webFriendsApi>; teams: ReturnType<typeof webTeamsApi> }>();
  useEffect(() => {
    const db = teamsDatabase(auth.app), current = { friends: webFriendsApi(user, db), teams: webTeamsApi(user, db) };
    setApis(current);
    return () => { current.friends.close(); current.teams.close(); };
  }, [user]);
  return apis ? <FriendsWorkspaceView friends={apis.friends} teams={apis.teams} uid={user.uid} zh={zh} route={route} chat={chat} code={code} onCodeHandled={onCodeHandled} onBadge={onBadge} /> : null;
}

/** A friend to open from `/friends?chat=<uid>`; the same shape the deep link allows. */
const chatParameter = (): string | undefined => {
  const value = new URLSearchParams(location.search).get('chat');
  return value && /^[A-Za-z0-9_-]{1,128}$/u.test(value) ? value : undefined;
};

export default function FriendsPage({ locale, route, badge, onBadge }: { locale: Locale } & Page) {
  const zh = locale === 'zh';
  const [user, setUser] = useState<User | null | undefined>(undefined);
  const [code, setCode] = useState(() => readAddIntent(`https://lingxios.net${location.pathname}${location.search}${location.hash}`, intentStorage));
  const handled = useRef(code), chat = useRef(chatParameter());
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    // Strip the link only after the continuation intent is retained. A browser denying storage can still recover it from the URL after a refresh.
    let retained = !code;
    if (code) retained = retainAddIntent(code, intentStorage);
    if (retained && (location.search || location.hash)) history.replaceState(null, '', location.pathname);
    return onAuthStateChanged(auth, setUser);
  }, []);
  const signOutNow = async () => {
    setBusy(true);
    try { await signOut(auth); } catch { /* The person stays signed in. */ }
    finally { setBusy(false); }
  };
  const done = () => { clearAddIntent(intentStorage, handled.current); setCode(''); };
  return <main className="teams-web"><header className="teams-web-nav"><a href="/">LingXi</a><h1>{zh ? '好友' : 'Friends'}{badge ? <b className="teams-web-badge" role="img" aria-label={zh ? `${badge} 条未读` : `${badge} unread`}>{badge}</b> : null}</h1><a href="/teams">{zh ? '团队' : 'Teams'}</a><span>{user?.displayName || user?.email}</span>{user ? <button disabled={busy} onClick={() => void signOutNow()}>{zh ? '退出登录' : 'Sign out'}</button> : null}</header>
    {user === undefined ? <p role="status" className="teams-web-note">{zh ? '正在加载…' : 'Loading…'}</p> : user
      ? <SignedIn key={user.uid} user={user} zh={zh} route={route} chat={chat.current} code={code} onCodeHandled={done} onBadge={onBadge} />
      : <SignInCard auth={auth} zh={zh} note={zh ? '请登录灵犀账号以添加好友并聊天。' : 'Sign in to your LingXi account to add friends and chat.'} />}
  </main>;
}
