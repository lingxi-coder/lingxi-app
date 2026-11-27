import { useEffect, useRef, useState } from 'react';
import { signOut, type User } from 'firebase/auth';
import { useAuthUser } from '../app/useAuthUser';
import { teamsDatabase } from '../teams/firestore';
import { auth } from '../app/firebase';
import { webFriendsApi } from '../friends/api';
import { webTeamsApi } from '../teams/api';
import { SignInCard } from '../teams/SignInCard';
import { TeamsWorkspaceView } from '../teams/TeamsWorkspaceView';
import { clearInviteIntent, readInviteIntent, retainInviteIntent } from '../teams/invite-intent';
import { buildDeepLink } from '../../../../packages/deep-links';
import type { ChannelInvite } from '../../../../packages/teams/channel-settings';
import { ChatTopbarTarget } from '../../../../packages/teams/ChatTopbar';
import { useTeamsText } from '../../../../packages/teams/i18n';
import type { Locale } from '../utils/locale';

const inviteStorage = {
  getItem(key: string) { return localStorage.getItem(key); },
  setItem(key: string, value: string) { localStorage.setItem(key, value); },
  removeItem(key: string) { localStorage.removeItem(key); },
};

function SignedIn({ user, locale, invite, channelInvite, onChannelInviteHandled, onInviteHandled, onInviteResolved }: { user: User; locale: string; invite: string; channelInvite?: ChannelInvite; onChannelInviteHandled(): void; onInviteHandled(): void; onInviteResolved(): void }) {
  const [apis, setApis] = useState<{ teams: ReturnType<typeof webTeamsApi>; friends: ReturnType<typeof webFriendsApi> }>();
  useEffect(() => {
    const db = teamsDatabase(auth.app), current = { teams: webTeamsApi(user, db), friends: webFriendsApi(user, db) };
    setApis(current);
    return () => { current.teams.close(); current.friends.close(); };
  }, [user]);
  return apis ? <TeamsWorkspaceView api={apis.teams} friends={apis.friends} uid={user.uid} locale={locale} invite={invite} channelInvite={channelInvite} onChannelInviteHandled={onChannelInviteHandled} onInviteHandled={onInviteHandled} onInviteResolved={onInviteResolved} /> : null;
}
export default function TeamsPage({ locale }: { locale: Locale }) {
  const t = useTeamsText(locale);
  const user = useAuthUser();
  const [code, setCode] = useState(() => readInviteIntent(`https://lingxios.net${location.pathname}${location.search}${location.hash}`, inviteStorage));
  const handledInvite = useRef(code);
  const [channelInvite, setChannelInvite] = useState<ChannelInvite | undefined>(() => {
    const query = new URLSearchParams(location.search), teamId = query.get('channelTeam'), conversationId = query.get('channel'), code = query.get('channelCode');
    const id = /^[A-Za-z0-9_-]{8,64}$/u;
    return teamId && id.test(teamId) && conversationId && id.test(conversationId) && code && /^[A-Za-z0-9_-]{32}$/u.test(code)
      ? { teamId, conversationId, code } : undefined;
  });
  const [chatTopbar, setChatTopbar] = useState<HTMLDivElement | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    // Strip the link only after the continuation intent is retained. A browser
    // denying storage can still recover it from the URL after a refresh.
    let retained = !code;
    if (code) retained = retainInviteIntent(code, inviteStorage);
    if (code && retained && (location.search || location.hash)) history.replaceState(null, '', location.pathname);
  }, []);
  const signOutNow = async () => {
    setBusy(true);
    try { await signOut(auth); } catch { /* The person stays signed in. */ }
    finally { setBusy(false); }
  };
  return <ChatTopbarTarget.Provider value={chatTopbar}><main className="teams-web"><header className="teams-web-nav"><a href="/">LingXi</a><a href="/inbox">{locale === 'zh' ? '消息' : 'Messages'}</a><a href="/friends">{t('friends_title')}</a><h1>{t('teams_title')}</h1><a href="/remote">{t('tw_nav_remote')}</a><div ref={setChatTopbar} className="teams-web-chat-slot" /><span>{user?.displayName || user?.email}</span>{user ? <button disabled={busy} onClick={() => void signOutNow()}>{t('tw_sign_out')}</button> : null}</header>
    {channelInvite && /Macintosh|Windows|X11/u.test(navigator.userAgent) ? <p className="teams-web-note"><a href={buildDeepLink({
      route: 'team/join-channel', params: { team: channelInvite.teamId, conversation: channelInvite.conversationId, code: channelInvite.code },
    }, 'scheme')}>{t('tw_channel_open_desktop')}</a></p> : null}
    {user === undefined ? <p role="status" className="teams-web-note">{t('teams_loading')}</p> : user ? <SignedIn key={user.uid} user={user} locale={locale} invite={code} channelInvite={channelInvite} onChannelInviteHandled={() => { setChannelInvite(undefined); history.replaceState(null, '', location.pathname); }} onInviteHandled={() => setCode('')} onInviteResolved={() => { clearInviteIntent(inviteStorage, handledInvite.current); setCode(''); history.replaceState(null, '', location.pathname); }} /> : <SignInCard auth={auth} locale={locale} note={code || channelInvite ? t('tw_signin_note_invite') : t('tw_signin_note_teams')} />}
  </main></ChatTopbarTarget.Provider>;
}
