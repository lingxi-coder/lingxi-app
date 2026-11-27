import {noticeTargetFromSearch} from '../inbox/route';
import { useState } from 'react';
import { Markdown } from '../remote/markdown';
import type { TeamMember, TeamsApi } from '../../../../packages/teams';
import { InviteNotice } from '../../../../packages/teams/friend-dialogs';
import type { FriendsApi } from '../../../../packages/teams/friends';
import { ChatTopbarControl } from '../../../../packages/teams/ChatTopbar';
import { useTeamsText } from '../../../../packages/teams/i18n';
import '../../../../packages/teams/styles';
import { ChannelJoinDialog } from '../../../../packages/teams/ChannelManagement';
import type { ChannelInvite } from '../../../../packages/teams/channel-settings';
import { TeamConversationView } from '../../../../packages/teams/TeamConversationView';
import { TeamDetailsDrawer } from '../../../../packages/teams/TeamDetailsDrawer';
import { TeamSidebar } from '../../../../packages/teams/TeamSidebar';
import type { DrawerTab, DrawerTarget, RenderMarkdown } from '../../../../packages/teams/ui';
import { useTeamsWorkspace, type UseTeams } from '../../../../packages/teams/useTeamsWorkspace';
import { InviteFriends } from '../friends/InviteFriends';
import { MemberFriend } from '../friends/MemberFriend';
import { renderTeamAvatar } from './teamAvatar';
import './teams-web.css';

/** A shared Bot's messages and documents use the Markdown renderer of Web's own chat (`Transcript`): React elements only, links limited to http(s) and mailto. */
const renderMarkdown: RenderMarkdown = text => <Markdown text={text} />;

interface LayoutProps {
  locale: string;
  /** An invitation from a link: the join dialog opens with it filled in, but never joins by itself. */
  invite?: string;
  channelInvite?: ChannelInvite;
  onChannelInviteHandled?(): void;
  onInviteHandled?(): void;
  onInviteResolved?(): void;
  /** Without it members have no Add friend action (D15) and owners and admins no Invite friends action (D26). */
  friends?: FriendsApi;
}

/** The Teams workspace of a signed-in account, opened with it and closed when the account or the page goes. */
export function TeamsWorkspaceView({ api, uid, ...layout }: LayoutProps & { api: TeamsApi; uid: string }) {
  const query = new URLSearchParams(typeof location==='undefined'?'':location.search);
  const teams = useTeamsWorkspace(api, uid, {initialTeamId: query.get('team') ?? undefined, initialConversationId: query.get('chat') ?? undefined});
  return <TeamsWorkspaceLayout teams={teams} {...layout} />;
}

/**
 * The team's conversations on the left, the open conversation in the middle and, when asked for, the team's details
 * on the right. Under 760px wide only one of the sidebar and the conversation is shown at a time. Wrap it in
 * `.teams-web`, which sets the workspace's colors from the site's theme.
 */
export function TeamsWorkspaceLayout({ teams: { workspace, snapshot }, locale, invite, channelInvite, onChannelInviteHandled, onInviteHandled, onInviteResolved, friends }: LayoutProps & { teams: UseTeams }) {
  const t = useTeamsText(locale);
  const [drawer, setDrawer] = useState<DrawerTarget>();
  const [teamTab, setTeamTab] = useState<DrawerTab>('members');
  const [conversationTab, setConversationTab] = useState<DrawerTab>('members');
  const [adding, setAdding] = useState<TeamMember>();
  const [inviting, setInviting] = useState(false);
  const [invited, setInvited] = useState<string[]>();
  const [pane, setPane] = useState<'list' | 'conversation'>();
  if (!workspace || !snapshot) return <p role="status" className="teams-web-note">{t('teams_loading')}</p>;
  const activePane = pane ?? (snapshot.team ? 'conversation' : 'list');
  return (
    <div className="teams-web-body" data-pane={activePane}>
      <div className="teams-web-sidebar">
        <TeamSidebar workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar} initialInvite={invite} onInviteHandled={onInviteHandled} onInviteResolved={() => { setPane('conversation'); onInviteResolved?.(); }}
          onOpenSettings={tab => { if (tab) setTeamTab(tab); setDrawer('team'); }} onConversationSelected={() => setPane('conversation')} />
      </div>
      <div className="teams-web-conversation">
        {activePane === 'conversation' ? <ChatTopbarControl><button type="button" className="teams-web-back" onClick={() => {
          setPane('list');
          setDrawer(undefined);
        }}>{t('tw_back_conversations')}</button></ChatTopbarControl> : null}
        {invited ? <InviteNotice names={invited} locale={locale} onDone={() => setInvited(undefined)} /> : null}
        <TeamConversationView notificationTarget={noticeTargetFromSearch(typeof location==='undefined'?'':location.search)} workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar} renderMarkdown={renderMarkdown}
          onOpenChannelSettings={() => setDrawer(undefined)} detailsOpen={drawer === 'conversation'} onToggleDetails={() => setDrawer(current => current === 'conversation' ? undefined : 'conversation')} />
      </div>
      {channelInvite && onChannelInviteHandled ? <ChannelJoinDialog key={channelInvite.code} workspace={workspace} snapshot={snapshot} locale={locale} invite={channelInvite} onClose={onChannelInviteHandled} onJoined={() => setPane('conversation')} /> : null}
      {drawer ? <TeamDetailsDrawer target={drawer} workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar}
        tab={drawer === 'team' ? teamTab : conversationTab} onTabChange={drawer === 'team' ? setTeamTab : setConversationTab}
        onClose={() => setDrawer(undefined)} onAddFriend={friends ? setAdding : undefined} onInviteFriends={friends ? () => setInviting(true) : undefined} /> : null}
      {adding && friends && snapshot.team ? <MemberFriend friends={friends} teams={workspace.api} uid={snapshot.uid} teamId={snapshot.team.id} member={adding} locale={locale} onClose={() => setAdding(undefined)} /> : null}
      {inviting && friends && snapshot.team ? <InviteFriends friends={friends} teams={workspace.api} uid={snapshot.uid} team={snapshot.team} members={snapshot.directory} locale={locale}
        refreshMembers={() => void workspace.refresh()} onClose={names => { setInviting(false); setInvited(names.length ? names : undefined); }} /> : null}
    </div>
  );
}
