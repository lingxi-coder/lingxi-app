import { useEffect, useMemo, useState } from 'react';
import { onAuthStateChanged, signOut, type User } from 'firebase/auth';
import { auth } from '../app/firebase';
import { Markdown } from '../remote/markdown';
import { webServicesApi } from '../services/api';
import { SignInCard } from '../teams/SignInCard';
import { HostedServices } from '../../../../packages/teams/HostedServices';
import { serviceLink } from '../../../../packages/teams/hosted-service';
import type { RenderMarkdown } from '../../../../packages/teams/ui';
import type { Locale } from '../utils/locale';
import '../../../../packages/teams/styles';
import '../teams/teams-web.css';

const renderMarkdown: RenderMarkdown = text => <Markdown text={text} />;

function SignedIn({ user, zh, serviceId }: { user: User; zh: boolean; serviceId?: string }) {
  const api = useMemo(() => webServicesApi(user, () => auth.currentUser?.uid), [user]);
  useEffect(() => () => api.close(), [api]);
  return <div className="teams-web-services"><HostedServices api={api} uid={user.uid} zh={zh} renderMarkdown={renderMarkdown} initialServiceId={serviceId}
    link={id => serviceLink(location.origin, id)} /></div>;
}

/** `/services` and `/services/<id>`: the Bots people offer outside their Teams, for a signed-in account. A link opens the service it names; nothing is entered before the person agrees. */
export default function ServicesPage({ locale, serviceId }: { locale: Locale; serviceId?: string }) {
  const zh = locale === 'zh';
  const [user, setUser] = useState<User | null | undefined>(undefined), [busy, setBusy] = useState(false);
  useEffect(() => onAuthStateChanged(auth, setUser), []);
  const signOutNow = async () => { setBusy(true); try { await signOut(auth); } catch { /* The person stays signed in. */ } finally { setBusy(false); } };
  return <main className="teams-web"><header className="teams-web-nav"><a href="/">LingXi</a><a href="/teams">{zh ? '团队' : 'Teams'}</a><a href="/friends">{zh ? '好友' : 'Friends'}</a><h1>{zh ? 'Bot 服务' : 'Bot services'}</h1>
      <span>{user?.displayName || user?.email}</span>{user ? <button disabled={busy} onClick={() => void signOutNow()}>{zh ? '退出登录' : 'Sign out'}</button> : null}</header>
    {user === undefined ? <p role="status" className="teams-web-note">{zh ? '正在加载…' : 'Loading…'}</p>
      : user ? <SignedIn key={user.uid} user={user} zh={zh} serviceId={serviceId} />
      : <SignInCard auth={auth} zh={zh} note={serviceId ? (zh ? '登录后查看该 Bot 服务。登录不会让你进入任何服务。' : 'Sign in to look at this Bot service. Signing in does not enter you into anything.') : (zh ? '使用灵犀账号使用他人提供的 Bot。' : 'Use Bots other people offer, with your LingXi account.')} />}
  </main>;
}
