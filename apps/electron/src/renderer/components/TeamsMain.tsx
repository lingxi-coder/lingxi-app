import { TeamConversationView } from '../../../../../packages/teams/TeamConversationView';
import { TeamDetailsDrawer } from '../../../../../packages/teams/TeamDetailsDrawer';
import type { UseTeams } from '../bridge/useTeams';
import type { TeamsDrawer } from '../bridge/useTeamsDrawer';
import { tr } from '../i18n/locale';
import { renderTeamAvatar } from './TeamAvatar';
import { TeamsSignIn } from './TeamsSignIn';

/**
 * The Teams page: the open conversation, and the details of its team beside it when the panel is open. The sidebar
 * (`TeamsSidebar`) is the other half; both read the one workspace the window holds. This is a separate chunk, so
 * the conversation and the Bot cards in it load when the page is first shown and not when the window opens.
 */
export function TeamsMain({ teams, zh, drawer }: { teams: UseTeams; zh: boolean; drawer: TeamsDrawer }) {
  const { workspace, snapshot, signedIn } = teams;
  if (!signedIn) return <TeamsSignIn />;
  if (!workspace || !snapshot) return <p role="status" style={{ margin: 'auto' }}>{tr('desktop_teams_loading')}</p>;
  return (
    <div className="teams-page">
      <div className="teams-conversation">
        <TeamConversationView workspace={workspace} snapshot={snapshot} zh={zh} renderAvatar={renderTeamAvatar}
          detailsOpen={drawer.open} onToggleDetails={drawer.toggle} />
      </div>
      {drawer.open ? <TeamDetailsDrawer workspace={workspace} snapshot={snapshot} zh={zh} renderAvatar={renderTeamAvatar}
        tab={drawer.tab} onTabChange={drawer.setTab} width={drawer.width} onWidthChange={drawer.setWidth} onClose={drawer.close} /> : null}
    </div>
  );
}
