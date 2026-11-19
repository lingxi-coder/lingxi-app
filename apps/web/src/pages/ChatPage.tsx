import { auth } from '../app/firebase';
import { useAuthUser } from '../app/useAuthUser';
import type { ChatTarget } from '../app/router';
import { ChatWorkspace } from '../chat/ChatWorkspace';
import { HomePane } from '../chat/panes/HomePane';
import { SignInCard } from '../teams/SignInCard';
import type { Locale } from '../utils/locale';
import '../teams/teams-web.css';

interface Props {
  locale: Locale;
  target: ChatTarget;
  theme: 'light' | 'dark';
  onLocaleChange(locale: Locale): void;
  onThemeToggle(): void;
}

/** `/chat…`: the signed-in workspace. Signed out, sign-in happens in place so a shared link continues at the same URL. */
export default function ChatPage({ locale, target, theme, onLocaleChange, onThemeToggle }: Props) {
  const user = useAuthUser();
  const zh = locale === 'zh';
  if (user === undefined) return <main className="teams-web" role="status"><p className="teams-web-note">{zh ? '正在加载…' : 'Loading…'}</p></main>;
  if (!user) return <main className="teams-web chat-signin">
    <a className="chat-side-brand" href="/?home"><img src="/brand/lockup-bilingual-ink.svg" alt="灵犀 LingXi" width={120} height={28} /></a>
    <SignInCard auth={auth} locale={locale} note={zh ? '登录后继续对话' : 'Sign in to continue to your chats'} />
  </main>;
  const name = user.displayName || user.email?.split('@')[0] || (zh ? '你好' : 'there');
  return <ChatWorkspace key={user.uid} user={user} locale={locale} theme={theme} target={target} onLocaleChange={onLocaleChange} onThemeToggle={onThemeToggle}
    renderAgent={() => <HomePane locale={locale} name={name} />} />;
}
