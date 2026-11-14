import { useMemo, useState } from 'react';
import type { TeamMember } from '../../../../../packages/teams';
import { InviteFriendsDialog, InviteNotice, MemberFriendDialog } from '../../../../../packages/teams/friend-dialogs';
import { HostedServices } from '../../../../../packages/teams/HostedServices';
import { serviceLink, type HostedServicesApi } from '../../../../../packages/teams/hosted-service';
import { TeamConversationView } from '../../../../../packages/teams/TeamConversationView';
import { TeamDetailsDrawer } from '../../../../../packages/teams/TeamDetailsDrawer';
import type { UseFriends } from '../../../../../packages/teams/useFriendsWorkspace';
import type { UseTeams } from '../../../../../packages/teams/useTeamsWorkspace';
import type { TeamsDrawer } from '../bridge/useTeamsDrawer';
import type { RenderMarkdown } from '../../../../../packages/teams/ui';
import { tr } from '../i18n/locale';
import { MarkdownContent } from './MarkdownContent';
import { renderTeamAvatar } from './TeamAvatar';
import { TeamsSignIn } from './TeamsSignIn';

/**
 * A shared Bot's messages and documents are another person's output, so they go through the same renderer as the
 * assistant's chat messages (code blocks, tables, Mermaid, sanitized links) with the two things that renderer can
 * switch on for trusted content left off: raw HTML (`trustedHtml`) and Bot-proposal cards (`botProposals`). Links are
 * limited to http(s) (`externalLinksOnly`): no app-internal mention links that open local files, no relative paths.
 */
export const renderTeamMarkdown: RenderMarkdown = text => <MarkdownContent text={text} trustedHtml={false} botProposals={false} externalLinksOnly />;

/**
 * The Teams page: the open conversation, and the details of its team beside it when the panel is open. The sidebar
 * (`TeamsSidebar`) is the other half; both read the one workspace the window holds. This is a separate chunk, so
 * the conversation and the Bot cards in it load when the page is first shown and not when the window opens.
 * A member of the team can be added as a friend (D15) and, by a manager, friends can be invited to it (D26) once the window has a
 * Friends workspace. `locale` orders the friends of the picker as the Friends list is ordered (D5).
 */
export function TeamsMain({ teams, friends, zh, locale, drawer, services, onCloseServices }: { teams: UseTeams; friends: UseFriends; zh: boolean; locale: string; drawer: TeamsDrawer;
  /** Bot services are shown in place of the Team. */ services: boolean; onCloseServices(): void }) {
  const { workspace, snapshot, signedIn } = teams;
  const [adding, setAdding] = useState<TeamMember>();
  const [inviting, setInviting] = useState(false);
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
        <button type="button" className="tw-link" style={{ alignSelf: 'flex-start', margin: '10px 20px 0' }} onClick={onCloseServices}>{zh ? '‹ 返回团队' : '‹ Back to the Team'}</button>
        <div style={{ flex: 1, minHeight: 0 }}>
          <HostedServices key={snapshot.uid} api={servicesApi} uid={snapshot.uid} zh={zh}
            renderMarkdown={renderTeamMarkdown} link={id => serviceLink('https://lingxios.net', id)} />
        </div>
      </div>
    </div>
  );
  return (
    <div className="teams-page">
      <div className="teams-conversation">
        {invited ? <InviteNotice names={invited} zh={zh} onDone={() => setInvited(undefined)} /> : null}
        <TeamConversationView workspace={workspace} snapshot={snapshot} zh={zh} renderAvatar={renderTeamAvatar} renderMarkdown={renderTeamMarkdown}
          detailsOpen={drawer.open} onToggleDetails={drawer.toggle} />
      </div>
      {drawer.open ? <TeamDetailsDrawer workspace={workspace} snapshot={snapshot} zh={zh} renderAvatar={renderTeamAvatar}
        tab={drawer.tab} onTabChange={drawer.setTab} width={drawer.width} onWidthChange={drawer.setWidth} onClose={drawer.close}
        onAddFriend={friends.workspace ? setAdding : undefined} onInviteFriends={friends.workspace ? () => setInviting(true) : undefined} /> : null}
      {adding && friends.workspace && snapshot.team ? <MemberFriendDialog workspace={friends.workspace} zh={zh} teamId={snapshot.team.id} member={adding}
        renderAvatar={renderTeamAvatar} onClose={() => setAdding(undefined)} /> : null}
      {inviting && friends.workspace && snapshot.team ? <InviteFriendsDialog workspace={friends.workspace} zh={zh} locale={locale} team={snapshot.team} members={snapshot.directory}
        renderAvatar={renderTeamAvatar} refreshMembers={() => void workspace.refresh()} onClose={names => { setInviting(false); setInvited(names.length ? names : undefined); }} /> : null}
    </div>
  );
}
