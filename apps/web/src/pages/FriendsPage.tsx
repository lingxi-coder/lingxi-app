import { useEffect, useRef, useState } from 'react';
import { onAuthStateChanged, signOut, type User } from 'firebase/auth';
import { auth } from '../app/firebase';
import { webFriendsApi } from '../friends/api';
import { clearAddIntent, readAddIntent, retainAddIntent } from '../friends/add-intent';
import { FriendsWorkspaceView } from '../friends/FriendsWorkspaceView';
import { SignInCard } from '../teams/SignInCard';
import { teamsDatabase } from '../teams/firestore';
import { webTeamsApi } from '../teams/api';
import { ChatTopbarTarget } from '../../../../packages/teams/ChatTopbar';
import { useTeamsText } from '../../../../packages/teams/i18n';
import type { Locale } from '../utils/locale';

const intentStorage = {
  getItem(key: string) { return localStorage.getItem(key); },
  setItem(key: string, value: string) { localStorage.setItem(key, value); },
  removeItem(key: string) { localStorage.removeItem(key); },
};

type Page = { route: '/friends' | '/friends/add'; badge: string | null; onBadge(text: string | null): void };

function SignedIn({ user, locale, route, chat, code, onCodeHandled, onBadge }: { user: User; locale: string; chat?: string; code: string; onCodeHandled(): void } & Pick<Page, 'route' | 'onBadge'>) {
  const [apis, setApis] = useState<{ friends: ReturnType<typeof webFriendsApi>; teams: ReturnType<typeof webTeamsApi> }>();
  useEffect(() => {
    const db = teamsDatabase(auth.app), current = { friends: webFriendsApi(user, db), teams: webTeamsApi(user, db) };
    setApis(current);
    return () => { current.friends.close(); current.teams.close(); };
  }, [user]);
  return apis ? <FriendsWorkspaceView friends={apis.friends} teams={apis.teams} uid={user.uid} locale={locale} route={route} chat={chat} code={code} onCodeHandled={onCodeHandled} onBadge={onBadge} /> : null;
}

/** A friend to open from `/friends?chat=<uid>`; the same shape the deep link allows. */
const chatParameter = (): string | undefined => {
  const value = new URLSearchParams(location.search).get('chat');
  return value && /^[A-Za-z0-9_-]{1,128}$/u.test(value) ? value : undefined;
};

export default function FriendsPage({ locale, route, badge, onBadge }: { locale: Locale } & Page) {
  const t = useTeamsText(locale);
  const [user, setUser] = useState<User | null | undefined>(undefined);
  const [code, setCode] = useState(() => readAddIntent(`https://lingxios.net${location.pathname}${location.search}${location.hash}`, intentStorage));
  const handled = useRef(code), chat = useRef(chatParameter());
  const [chatTopbar, setChatTopbar] = useState<HTMLDivElement | null>(null);
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
  return <ChatTopbarTarget.Provider value={chatTopbar}><main className="teams-web"><header className="teams-web-nav"><a href="/">LingXi</a><h1>{t('friends_title')}{badge ? <b className="teams-web-badge" role="img" aria-label={t('tw_unread_fmt', badge)}>{badge}</b> : null}</h1><a href="/teams">{t('teams_title')}</a><a href="/remote">{t('tw_nav_remote')}</a><div ref={setChatTopbar} className="teams-web-chat-slot" /><span>{user?.displayName || user?.email}</span>{user ? <button disabled={busy} onClick={() => void signOutNow()}>{t('tw_sign_out')}</button> : null}</header>
    {user === undefined ? <p role="status" className="teams-web-note">{t('teams_loading')}</p> : user
      ? <SignedIn key={user.uid} user={user} locale={locale} route={route} chat={chat.current} code={code} onCodeHandled={done} onBadge={onBadge} />
      : <SignInCard auth={auth} locale={locale} note={t('friends_signin')} />}
  </main></ChatTopbarTarget.Provider>;
}
