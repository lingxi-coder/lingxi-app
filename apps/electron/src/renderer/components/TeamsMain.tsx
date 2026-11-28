import { useMemo, useState } from 'react';
import type { TeamMember } from '../../../../../packages/teams';
import { InviteFriendsDialog, InviteNotice, MemberFriendDialog } from '../../../../../packages/teams/friend-dialogs';
import { HostedServices } from '../../../../../packages/teams/HostedServices';
import { serviceLink, type HostedServicesApi } from '../../../../../packages/teams/hosted-service';
import { ChannelJoinDialog } from '../../../../../packages/teams/ChannelManagement';
import type { ChannelInvite } from '../../../../../packages/teams/channel-settings';
import type { ChannelPostTarget } from '../../../../../packages/teams/share';
import { TeamConversationView } from '../../../../../packages/teams/TeamConversationView';
import { TeamDetailsDrawer } from '../../../../../packages/teams/TeamDetailsDrawer';
import { TeamSettingsPage } from '../../../../../packages/teams/TeamSettingsPage';
import { TeamSettingsSurface, type useTeamSettings } from '../../../../../packages/teams/TeamSettingsSurface';
import type { UseFriends } from '../../../../../packages/teams/useFriendsWorkspace';
import type { UseTeams } from '../../../../../packages/teams/useTeamsWorkspace';
import type { TeamsDrawer } from '../bridge/useTeamsDrawer';
import type { RenderMarkdown } from '../../../../../packages/teams/ui';
import { tr } from '../i18n/locale';
import { MarkdownContent } from './MarkdownContent';
import { renderTeamAvatar } from './TeamAvatar';
import { TeamsSignIn } from './TeamsSignIn';
import type { InboxNoticeTarget } from '../../../../../packages/inbox/contracts';

/**
 * A shared Bot's messages and documents are another person's output, so they go through the same renderer as the
 * assistant's chat messages (code blocks, tables, Mermaid, sanitized links) with the two things that renderer can
 * switch on for trusted content left off: raw HTML (`trustedHtml`) and Bot-proposal cards (`botProposals`). Links are
 * limited to http(s) (`externalLinksOnly`): no app-internal mention links that open local files, no relative paths.
 */
export const renderTeamMarkdown: RenderMarkdown = text => <MarkdownContent text={text} trustedHtml={false} botProposals={false} externalLinksOnly />;

/**
 * The Teams page: the open conversation and its details panel. The sidebar
 * (`TeamsSidebar`) is the other half; both read the one workspace the window holds. This is a separate chunk, so
 * the conversation and the Bot cards in it load when the page is first shown and not when the window opens.
 * A member of the team can be added as a friend (D15) and, by a manager, friends can be invited to it (D26) once the window has a
 * Friends workspace. `locale` orders the friends of the picker as the Friends list is ordered (D5).
 */
export function TeamsMain({ teams, friends, locale, drawer, settings, services, inboxTarget, notificationTarget, sharedPost, onExitPost, channelInvite, onChannelInviteHandled, onCloseServices }: { teams: UseTeams; friends: UseFriends; locale: string; drawer: TeamsDrawer;
  settings: ReturnType<typeof useTeamSettings>;
  inboxTarget?: { serviceId: string; sessionId: string };
  notificationTarget?: InboxNoticeTarget;
  sharedPost?: ChannelPostTarget; onExitPost?(): void;
  channelInvite?: ChannelInvite; onChannelInviteHandled?(): void;
  /** Bot services are shown in place of the Team. */ services: boolean; onCloseServices(): void }) {
  const { workspace, snapshot, signedIn } = teams;
  const [adding, setAdding] = useState<{ teamId: string; member: TeamMember }>();
  const [inviting, setInviting] = useState<string>();
  const [invited, setInvited] = useState<string[]>();
  // One object for as long as the account's workspace lives: the services page reads again whenever it changes.
  const servicesApi = useMemo<HostedServicesApi | undefined>(() => {
    const api = workspace?.api;
    return api?.services && api.serviceMessages ? { services: api.services, serviceMessages: api.serviceMessages, localBots: api.localBots, publishLocalBot: api.publishLocalBot, hostStatus: api.hostStatus } : undefined;
  }, [workspace]);
  if (!signedIn) return <TeamsSignIn />;
  if (!workspace || !snapshot) return <p role="status" style={{ margin: 'auto' }}>{tr('desktop_teams_loading')}</p>;
  if (services && servicesApi) return (
    <div className="teams-page">
      <div className="teams-conversation" style={{ display: 'flex', flexDirection: 'column', minHeight: 0 }}>
        <div style={{ flex: 1, minHeight: 0 }}>
          <HostedServices key={snapshot.uid} api={servicesApi} uid={snapshot.uid} locale={locale}
            notificationTarget={notificationTarget}
            initialServiceId={inboxTarget?.serviceId} initialSessionId={inboxTarget?.sessionId}
            renderMarkdown={renderTeamMarkdown} link={id => serviceLink('https://lingxios.net', id)} onBackToTeams={onCloseServices} />
        </div>
      </div>
    </div>
  );
  const addFriend = friends.workspace ? (member: TeamMember) => setAdding({ teamId: snapshot.selectedTeamId, member }) : undefined;
  const status = invited ? <InviteNotice names={invited} locale={locale} onDone={() => setInvited(undefined)} /> : null;
  return (
    <div className="teams-page">
      <TeamSettingsSurface settings={settings.active ? <TeamSettingsPage key={settings.active.id} initialSection={settings.active.section}
        workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar} status={status}
        onClose={settings.back} onOpenChat={settings.dismiss} onAddFriend={addFriend}
        onInviteFriends={friends.workspace ? () => setInviting(snapshot.selectedTeamId) : undefined} /> : undefined}>
        <div className="teams-conversation">
          {!settings.active ? status : null}
          <TeamConversationView workspace={workspace} snapshot={snapshot} locale={locale} notificationTarget={notificationTarget} renderAvatar={renderTeamAvatar} renderMarkdown={renderTeamMarkdown}
            sharedPostId={sharedPost && sharedPost.teamId === snapshot.team?.id && sharedPost.conversationId === snapshot.conversation?.id ? sharedPost.postId : undefined} onExitPost={onExitPost}
            onOpenTeamSettings={opener => settings.open('general', opener)} covered={!!settings.active}
            onOpenChannelSettings={drawer.close} detailsOpen={drawer.open} onToggleDetails={drawer.toggle} />
        </div>
        {drawer.open && snapshot.conversation ? <TeamDetailsDrawer snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar}
          width={drawer.width} onWidthChange={drawer.setWidth} onClose={drawer.close} onAddFriend={addFriend} /> : null}
      </TeamSettingsSurface>
      {channelInvite && onChannelInviteHandled ? <ChannelJoinDialog key={channelInvite.code} workspace={workspace} snapshot={snapshot} locale={locale} invite={channelInvite} onClose={onChannelInviteHandled} /> : null}
      {adding && friends.workspace && adding.teamId === snapshot.selectedTeamId ? <MemberFriendDialog workspace={friends.workspace} locale={locale} teamId={adding.teamId} member={adding.member}
        renderAvatar={renderTeamAvatar} onClose={() => setAdding(undefined)} /> : null}
      {inviting && friends.workspace && snapshot.team?.id === inviting ? <InviteFriendsDialog workspace={friends.workspace} locale={locale} team={snapshot.team} members={snapshot.directory}
        renderAvatar={renderTeamAvatar} refreshMembers={() => void workspace.refresh()} onClose={names => { setInviting(undefined); setInvited(names.length ? names : undefined); }} /> : null}
    </div>
  );
}
