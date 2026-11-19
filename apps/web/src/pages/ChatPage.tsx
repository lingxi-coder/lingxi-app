import { useEffect, useMemo, useState } from 'react';
import type { User } from 'firebase/auth';
import { auth } from '../app/firebase';
import { useAuthUser } from '../app/useAuthUser';
import type { ChatTarget } from '../app/router';
import { ChatWorkspace } from '../chat/ChatWorkspace';
import { AgentHost } from '../chat/agent/AgentPane';
import { createLlmClient, LlmError } from '../chat/agent/client';
import { openHistory, type AgentHistory } from '../chat/agent/history';
import type { AgentChatEntry } from '../chat/ChatSidebar';
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
    renderAgent={props => <AgentArea user={user} name={name} locale={locale} {...props} />} />;
}

/** A shared `new` link arrives as `/chat#prompt=…`: the text goes into the composer, and the fragment leaves the address. */
function takePrompt(): string | undefined {
  const match = /^#prompt=(.*)$/su.exec(window.location.hash);
  if (!match) return undefined;
  window.history.replaceState(window.history.state, '', window.location.pathname + window.location.search);
  try { return decodeURIComponent(match[1]!).slice(0, 20_000) || undefined; } catch { return undefined; }
}

function AgentArea({ user, name, locale, target, onAgentChats, open }: { user: User; name: string; locale: Locale; target: ChatTarget; onAgentChats(chats: AgentChatEntry[]): void; open(target: ChatTarget, replace?: boolean): void }) {
  const client = useMemo(() => createLlmClient(async () => {
    if (auth.currentUser?.uid !== user.uid) throw new LlmError('INVALID_SESSION');
    const token = await user.getIdToken();
    if (auth.currentUser?.uid !== user.uid) throw new LlmError('INVALID_SESSION');
    return token;
  }), [user]);
  const [history, setHistory] = useState<AgentHistory>();
  const [prompt] = useState(takePrompt);
  useEffect(() => { let live = true; void openHistory().then(opened => { if (live) setHistory(opened); }); return () => { live = false; }; }, []);
  if (!history) return <div className="chat-notice" role="status"><p>{locale === 'zh' ? '正在加载…' : 'Loading…'}</p></div>;
  return <AgentHost uid={user.uid} name={name} locale={locale} target={target} client={client} history={history} onAgentChats={onAgentChats} open={open} initialPrompt={prompt} />;
}
