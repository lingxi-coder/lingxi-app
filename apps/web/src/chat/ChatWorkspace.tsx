import { useCallback, useEffect, useState, useSyncExternalStore, type ReactNode } from 'react';
import { signOut, type User } from 'firebase/auth';
import * as sdk from 'firebase/firestore';
import { InboxController, type InboxSnapshot } from '../../../../packages/inbox/controller';
import { watchInbox } from '../../../../packages/inbox/firestore';
import '../../../../packages/teams/styles';
import { useFriendsWorkspace } from '../../../../packages/teams/useFriendsWorkspace';
import { useTeamsWorkspace } from '../../../../packages/teams/useTeamsWorkspace';
import { auth } from '../app/firebase';
import { chatRoute, navigateTo, resolveRoute, type ChatTarget } from '../app/router';
import { webFriendsApi } from '../friends/api';
import { webInboxRequest } from '../inbox/session';
import { webTeamsApi } from '../teams/api';
import { teamsDatabase } from '../teams/firestore';
import { cameViaLink, sharedPrompt, withoutViaLink } from '../pages/openLinkTarget';
import type { Locale } from '../utils/locale';
import { OpenInAppBanner } from './OpenInAppBanner';
import type { AgentChatSummary } from './agent/history';
import { ChatSidebar } from './ChatSidebar';
import { BotPane } from './panes/BotPane';
import { FriendPane } from './panes/FriendPane';
import { TeamPane } from './panes/TeamPane';
import './chat.css';

/** A friendship's chat lives in a pair team, whose id starts with `f_`; links name it so both friends can open them. */
export const isPairTeam = (teamId: string) => teamId.startsWith('f_');

const noInbox = (): (() => void) => () => undefined;
const noSnapshot = (): InboxSnapshot | undefined => undefined;

export interface ChatWorkspaceProps {
  user: User;
  locale: Locale;
  theme: 'light' | 'dark';
  target: ChatTarget;
  onLocaleChange(locale: Locale): void;
  onThemeToggle(): void;
  /** The saved Agent chats, for the sidebar whichever pane is open; an open Agent pane keeps the list current after that. */
  listAgentChats?(): Promise<AgentChatSummary[]>;
  /** The centre of the workspace for the targets the people panes do not handle: the Agent chat and model settings. */
  renderAgent(props: { target: ChatTarget; onAgentChats(chats: AgentChatSummary[]): void; open(target: ChatTarget, replace?: boolean): void }): ReactNode;
}

/** Move inside the Chat workspace; `replace` follows a change without a new history entry. */
export function openChat(target: ChatTarget, replace = false): void {
  const route = chatRoute(target);
  if (!replace) { navigateTo(route); return; }
  if (resolveRoute(window.location.pathname) === route) return;
  window.history.replaceState({}, '', route);
  window.dispatchEvent(new PopStateEvent('popstate'));
}

export function ChatWorkspace({ user, locale, theme, target, onLocaleChange, onThemeToggle, listAgentChats, renderAgent }: ChatWorkspaceProps) {
  const zh = locale === 'zh';
  const [apis, setApis] = useState<{ teams: ReturnType<typeof webTeamsApi>; friends: ReturnType<typeof webFriendsApi>; inbox: InboxController }>();
  const [agentChats, setAgentChats] = useState<AgentChatSummary[]>([]);
  const [drawer, setDrawer] = useState(false);
  // A shared link that continued here offers the app once, with the prompt a new-chat link brought; the marker leaves the
  // address straight away.
  const [viaLink, setViaLink] = useState(() => cameViaLink(window.location.search) ? { prompt: sharedPrompt(window.location.hash) } : undefined);
  useEffect(() => {
    if (!cameViaLink(window.location.search)) return;
    window.history.replaceState(window.history.state, '', `${window.location.pathname}${withoutViaLink(window.location.search)}${window.location.hash}`);
  }, []);
  useEffect(() => {
    let live = true;
    void listAgentChats?.().then(list => { if (live) setAgentChats(list); }, () => undefined);
    return () => { live = false; };
  }, [listAgentChats]);
  useEffect(() => {
    const db = teamsDatabase(auth.app);
    const inbox = new InboxController({ uid: user.uid, request: webInboxRequest(user), watch: (...args) => watchInbox(sdk, db, user.uid, ...args) });
    const current = { teams: webTeamsApi(user, db), friends: webFriendsApi(user, db), inbox };
    setApis(current);
    void inbox.start();
    return () => { inbox.close(); current.teams.close(); current.friends.close(); };
  }, [user]);
  const inbox = useSyncExternalStore(apis ? apis.inbox.subscribe : noInbox, apis ? apis.inbox.getSnapshot : noSnapshot);

  // The Teams and Friends workspaces open the first time a pane needs them, and stay open while the page lives.
  // A friendship's pair team (`f_…`) opens in Friends, whether the link names its chat or a Bot (as on every client).
  const pair = (target.kind === 'team' || target.kind === 'bot') && isPairTeam(target.teamId);
  const wantsTeams = !pair && (target.kind === 'bot' || target.kind === 'team');
  const wantsFriends = pair || target.kind === 'friend';
  const [needed, setNeeded] = useState({ teams: wantsTeams, friends: wantsFriends });
  useEffect(() => { if ((wantsTeams && !needed.teams) || (wantsFriends && !needed.friends)) setNeeded(current => ({ teams: current.teams || wantsTeams, friends: current.friends || wantsFriends })); }, [wantsTeams, wantsFriends, needed]);
  const teams = useTeamsWorkspace(needed.teams ? apis?.teams : undefined, user.uid);
  const friends = useFriendsWorkspace(needed.friends ? apis?.friends : undefined, apis?.teams, user.uid);

  const open = useCallback((next: ChatTarget, replace = false) => { setDrawer(false); openChat(next, replace); }, []);
  const follow = useCallback((next: ChatTarget) => openChat(next, true), []);

  let pane: ReactNode;
  if (pair) pane = <FriendPane friends={friends} pairTeamId={target.teamId} locale={locale} />;
  else if (target.kind === 'team') pane = <TeamPane teams={teams} teamId={target.teamId} conversationId={target.conversationId} locale={locale} friends={apis?.friends} onFollow={follow} />;
  else if (target.kind === 'friend') pane = <FriendPane friends={friends} uid={target.uid} locale={locale} />;
  else if (target.kind === 'bot') pane = <BotPane teams={teams} teamId={target.teamId} botId={target.botId} locale={locale} onFollow={open} />;
  else pane = renderAgent({ target, onAgentChats: setAgentChats, open });

  return <div className="chat-shell teams-web" data-drawer={drawer ? 'open' : undefined}>
    <ChatSidebar locale={locale} theme={theme} target={target} rows={inbox?.items ?? []} rowsError={inbox?.error || undefined} agentChats={agentChats}
      account={user.displayName || user.email || (zh ? '我的账号' : 'My account')}
      onNewChat={() => open({ kind: 'home' })} onOpenAgent={id => open({ kind: 'agent', id })}
      onOpenRow={destination => destination.kind === 'chat' ? open(destination.target) : window.location.assign(destination.href)}
      onOpen={href => href.startsWith('/chat') ? navigateTo(resolveRoute(href)) : window.location.assign(href)}
      onLocaleChange={onLocaleChange} onThemeToggle={onThemeToggle} onSignOut={() => void signOut(auth).catch(() => undefined)} />
    <button type="button" className="chat-drawer-scrim" aria-label={zh ? '关闭对话列表' : 'Close the conversation list'} onClick={() => setDrawer(false)} />
    <main className="chat-main">
      <button type="button" className="chat-drawer-toggle" aria-label={zh ? '打开对话列表' : 'Open the conversation list'} aria-expanded={drawer} onClick={() => setDrawer(true)}><span /><span /><span /></button>
      {viaLink && <OpenInAppBanner target={target} prompt={viaLink.prompt} locale={locale} onClose={() => setViaLink(undefined)} />}
      {pane}
    </main>
  </div>;
}

