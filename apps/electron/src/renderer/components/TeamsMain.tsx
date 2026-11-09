import { TeamConversationView } from '../../../../../packages/teams/TeamConversationView';
import { TeamDetailsDrawer } from '../../../../../packages/teams/TeamDetailsDrawer';
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
const renderTeamMarkdown: RenderMarkdown = text => <MarkdownContent text={text} trustedHtml={false} botProposals={false} externalLinksOnly />;

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
        <TeamConversationView workspace={workspace} snapshot={snapshot} zh={zh} renderAvatar={renderTeamAvatar} renderMarkdown={renderTeamMarkdown}
          detailsOpen={drawer.open} onToggleDetails={drawer.toggle} />
      </div>
      {drawer.open ? <TeamDetailsDrawer workspace={workspace} snapshot={snapshot} zh={zh} renderAvatar={renderTeamAvatar}
        tab={drawer.tab} onTabChange={drawer.setTab} width={drawer.width} onWidthChange={drawer.setWidth} onClose={drawer.close} /> : null}
    </div>
  );
}
