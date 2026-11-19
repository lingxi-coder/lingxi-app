import {noticeTargetFromSearch} from '../inbox/route';
import { useEffect, useMemo, useState } from 'react';
import { signOut, type User } from 'firebase/auth';
import { useAuthUser } from '../app/useAuthUser';
import { auth } from '../app/firebase';
import { Markdown } from '../remote/markdown';
import { webServicesApi } from '../services/api';
import { SignInCard } from '../teams/SignInCard';
import { ChatTopbarTarget } from '../../../../packages/teams/ChatTopbar';
import { HostedServices } from '../../../../packages/teams/HostedServices';
import { serviceLink } from '../../../../packages/teams/hosted-service';
import { useTeamsText } from '../../../../packages/teams/i18n';
import type { RenderMarkdown } from '../../../../packages/teams/ui';
import type { Locale } from '../utils/locale';
import '../../../../packages/teams/styles';
import '../teams/teams-web.css';

const renderMarkdown: RenderMarkdown = text => <Markdown text={text} />;

function SignedIn({ user, locale, serviceId }: { user: User; locale: string; serviceId?: string }) {
  const api = useMemo(() => webServicesApi(user, () => auth.currentUser?.uid), [user]);
  useEffect(() => () => api.close(), [api]);
  return <div className="teams-web-services"><HostedServices notificationTarget={noticeTargetFromSearch(location.search)} api={api} uid={user.uid} locale={locale} renderMarkdown={renderMarkdown} initialServiceId={serviceId} initialSessionId={new URLSearchParams(location.search).get('session') ?? undefined}
    link={id => serviceLink(location.origin, id)} /></div>;
}

/** `/services` and `/services/<id>`: the Bots people offer outside their Teams, for a signed-in account. A link opens the service it names; nothing is entered before the person agrees. */
export default function ServicesPage({ locale, serviceId }: { locale: Locale; serviceId?: string }) {
  const t = useTeamsText(locale);
  const user = useAuthUser(), [busy, setBusy] = useState(false);
  const [chatTopbar, setChatTopbar] = useState<HTMLDivElement | null>(null);
  const signOutNow = async () => { setBusy(true); try { await signOut(auth); } catch { /* The person stays signed in. */ } finally { setBusy(false); } };
  return <ChatTopbarTarget.Provider value={chatTopbar}><main className="teams-web"><header className="teams-web-nav"><a href="/">LingXi</a><a href="/inbox">{locale === 'zh' ? '消息' : 'Messages'}</a><a href="/teams">{t('teams_title')}</a><a href="/friends">{t('friends_title')}</a><h1>{t('services_title')}</h1>
      <div ref={setChatTopbar} className="teams-web-chat-slot" />
      <span>{user?.displayName || user?.email}</span>{user ? <button disabled={busy} onClick={() => void signOutNow()}>{t('tw_sign_out')}</button> : null}</header>
    {user === undefined ? <p role="status" className="teams-web-note">{t('teams_loading')}</p>
      : user ? <SignedIn key={user.uid} user={user} locale={locale} serviceId={serviceId} />
      : <SignInCard auth={auth} locale={locale} note={serviceId ? t('tw_signin_note_service') : t('tw_signin_note_services')} />}
  </main></ChatTopbarTarget.Provider>;
}
