import { useCallback, useEffect, useMemo, useState } from 'react';
import type { User } from 'firebase/auth';
import { auth } from '../app/firebase';
import { DatasetPageCard } from '../../../../packages/teams/DatasetPage.js';
import { isResourceRef } from '../../../../packages/teams/dataset-resource.js';
import { teamsTransport } from '../../../../packages/teams/index.js';
import { useAuthUser } from '../app/useAuthUser';
import type { ChatTarget } from '../app/router';
import { ChatWorkspace } from '../chat/ChatWorkspace';
import { OpenInAppBanner } from '../chat/OpenInAppBanner';
import { AgentHost, useModelServices, type ModelServices } from '../chat/agent/AgentPane';
import { createLlmClient, LlmError, type LlmClient } from '../chat/agent/client';
import { openHistory, type AgentChatSummary, type AgentHistory } from '../chat/agent/history';
import { SignInCard } from '../teams/SignInCard';
import type { Locale } from '../utils/locale';
import { cameViaLink, sharedPrompt } from './openLinkTarget';
import '../teams/teams-web.css';

let pageHistory: Promise<AgentHistory> | undefined;
/**
 * One Agent chat history per page load: the sidebar lists it whichever pane is open, and every Agent pane reads and writes
 * the same one, so chats kept only in memory (no IndexedDB) survive a visit to a team or friend.
 */
const agentHistory = () => pageHistory ??= openHistory();

interface Props {
  locale: Locale;
  target: ChatTarget;
  theme: 'light' | 'dark';
  onLocaleChange(locale: Locale): void;
  onThemeToggle(): void;
}

/** `/chat…`: the signed-in workspace. Signed out, sign-in happens in place so a shared link continues at the same URL. */
export default function ChatPage(props: Props) {
  const { locale, target } = props;
  const user = useAuthUser();
  const zh = locale === 'zh';
  // A shared link that continued here still offers the app before sign-in; the workspace offers it again after.
  const [viaLink, setViaLink] = useState(() => cameViaLink(window.location.search) ? { prompt: sharedPrompt(window.location.hash) } : undefined);
  if (user === undefined) return <main className="teams-web" role="status"><p className="teams-web-note">{zh ? '正在加载…' : 'Loading…'}</p></main>;
  if (!user) return <main className="teams-web chat-signin">
    <a className="chat-side-brand" href="/?home"><img src="/brand/lockup-bilingual-ink.svg" alt="灵犀 LingXi" width={120} height={28} /></a>
    {viaLink ? <OpenInAppBanner target={target} prompt={viaLink.prompt} locale={locale} onClose={() => setViaLink(undefined)} /> : null}
    <SignInCard auth={auth} locale={locale} note={zh ? '登录后继续对话' : 'Sign in to continue to your chats'} />
  </main>;
  const query = new URLSearchParams(window.location.search);
  const reference = { kind: 'pageVersion', page: query.get('datasetPage'), version: query.get('datasetVersion') };
  if (isResourceRef(reference) && reference.kind === 'pageVersion') {
    return <SignedInDatasetPage key={user.uid + ':' + reference.version} user={user} locale={locale} reference={reference} />;
  }
  return <SignedInChat key={user.uid} user={user} {...props} />;
}

function useSignedInDatasetApi(user: User) {
  return useMemo(() => {
    const request = teamsTransport('', async () => {
      if (auth.currentUser?.uid !== user.uid) throw new Error('INVALID_SESSION');
      const token = await user.getIdToken();
      if (auth.currentUser?.uid !== user.uid) throw new Error('INVALID_SESSION');
      return token;
    });
    return { datasets: <T,>(body: Record<string, unknown>) => request<T>('/datasets', body) };
  }, [user]);
}

function SignedInDatasetPage({ user, locale, reference }: {
  user: User; locale: Locale; reference: { kind: 'pageVersion'; page: string; version: string };
}) {
  const api = useSignedInDatasetApi(user);
  return <main className="teams-web chat-signin">
    <div className="tw-scope">
      <DatasetPageCard api={api} reference={reference} locale={locale} />
    </div>
  </main>;
}

/** The workspace for one account: its model client, model services and shared prompt live as long as the page does. */
function SignedInChat({ user, locale, target, theme, onLocaleChange, onThemeToggle }: Props & { user: User }) {
  const client = useMemo(() => createLlmClient(async () => {
    if (auth.currentUser?.uid !== user.uid) throw new LlmError('INVALID_SESSION');
    const token = await user.getIdToken();
    if (auth.currentUser?.uid !== user.uid) throw new LlmError('INVALID_SESSION');
    return token;
  }), [user]);
  const models = useModelServices(client);
  // A shared `new` link arrives as `/chat#prompt=…`: the text goes into the composer once, and the fragment leaves the address.
  const [prompt, setPrompt] = useState(() => sharedPrompt(window.location.hash));
  useEffect(() => {
    if (/^#prompt=/u.test(window.location.hash)) window.history.replaceState(window.history.state, '', window.location.pathname + window.location.search);
  }, []);
  const takePrompt = useCallback(() => setPrompt(undefined), []);
  const listAgentChats = useCallback(() => agentHistory().then(history => history.list(user.uid)), [user.uid]);
  const name = user.displayName || user.email?.split('@')[0] || (locale === 'zh' ? '你好' : 'there');
  return <ChatWorkspace user={user} locale={locale} theme={theme} target={target} onLocaleChange={onLocaleChange} onThemeToggle={onThemeToggle}
    listAgentChats={listAgentChats} renderAgent={props => <AgentArea user={user} name={name} locale={locale} client={client} models={models}
      prompt={prompt} onPromptTaken={takePrompt} {...props} />} />;
}

interface AreaProps {
  user: User; name: string; locale: Locale; client: LlmClient; models: ModelServices; prompt?: string; onPromptTaken(): void;
  target: ChatTarget; onAgentChats(chats: AgentChatSummary[]): void; open(target: ChatTarget, replace?: boolean): void;
}

function AgentArea({ user, name, locale, client, models, prompt, onPromptTaken, target, onAgentChats, open }: AreaProps) {
  const [history, setHistory] = useState<AgentHistory>();
  const datasetApi = useSignedInDatasetApi(user);
  useEffect(() => { let live = true; void agentHistory().then(opened => { if (live) setHistory(opened); }); return () => { live = false; }; }, []);
  if (!history) return <div className="chat-notice" role="status"><p>{locale === 'zh' ? '正在加载…' : 'Loading…'}</p></div>;
  return <AgentHost uid={user.uid} name={name} locale={locale} target={target} client={client} datasetApi={datasetApi} models={models} history={history}
    onAgentChats={onAgentChats} open={open} initialPrompt={prompt} onPromptTaken={onPromptTaken} />;
}
