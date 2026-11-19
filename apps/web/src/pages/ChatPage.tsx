import { auth } from '../app/firebase';
import { useAuthUser } from '../app/useAuthUser';
import { SignInCard } from '../teams/SignInCard';
import type { ChatTarget } from '../app/router';
import type { Locale } from '../utils/locale';
import '../teams/teams-web.css';

/** `/chat…`: the signed-in workspace. Signed out, sign-in happens in place so a shared link continues at the same URL. */
export default function ChatPage({ locale }: { locale: Locale; target: ChatTarget }) {
  const user = useAuthUser();
  const zh = locale === 'zh';
  if (user === undefined) return <main className="teams-web" role="status">{zh ? '正在加载…' : 'Loading…'}</main>;
  if (!user) return <main className="teams-web"><SignInCard auth={auth} locale={locale} note={zh ? '登录后继续对话' : 'Sign in to continue to your chats'} /></main>;
  return <main className="teams-web"><p className="teams-web-note">{zh ? '对话工作台即将上线。' : 'Chat is coming soon.'}</p></main>;
}
