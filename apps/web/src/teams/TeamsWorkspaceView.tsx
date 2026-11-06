import { useState } from 'react';
import type { TeamsApi } from '../../../../packages/teams';
import '../../../../packages/teams/styles';
import { TeamConversationView } from '../../../../packages/teams/TeamConversationView';
import { TeamDetailsDrawer } from '../../../../packages/teams/TeamDetailsDrawer';
import { TeamSidebar } from '../../../../packages/teams/TeamSidebar';
import type { DrawerTab } from '../../../../packages/teams/ui';
import { useTeamsWorkspace, type UseTeams } from '../../../../packages/teams/useTeamsWorkspace';
import './teams-web.css';

interface LayoutProps {
  zh: boolean;
  /** An invitation from a link: the join dialog opens with it filled in, but never joins by itself. */
  invite?: string;
  onInviteHandled?(): void;
  onInviteResolved?(): void;
}

/** The Teams workspace of a signed-in account, opened with it and closed when the account or the page goes. */
export function TeamsWorkspaceView({ api, uid, ...layout }: LayoutProps & { api: TeamsApi; uid: string }) {
  const teams = useTeamsWorkspace(api, uid);
  return <TeamsWorkspaceLayout teams={teams} {...layout} />;
}

/**
 * The team's conversations on the left, the open conversation in the middle and, when asked for, the team's details
 * on the right. Under 760px wide only one of the sidebar and the conversation is shown at a time. Wrap it in
 * `.teams-web`, which sets the workspace's colors from the site's theme.
 */
export function TeamsWorkspaceLayout({ teams: { workspace, snapshot }, zh, invite, onInviteHandled, onInviteResolved }: LayoutProps & { teams: UseTeams }) {
  const [drawer, setDrawer] = useState<{ open: boolean; tab: DrawerTab }>({ open: false, tab: 'members' });
  const [pane, setPane] = useState<'list' | 'conversation'>();
  if (!workspace || !snapshot) return <p role="status" className="teams-web-note">{zh ? '正在加载…' : 'Loading…'}</p>;
  return (
    <div className="teams-web-body" data-pane={pane ?? (snapshot.team ? 'conversation' : 'list')}>
      <div className="teams-web-sidebar">
        <TeamSidebar workspace={workspace} snapshot={snapshot} zh={zh} initialInvite={invite} onInviteHandled={onInviteHandled} onInviteResolved={() => { setPane('conversation'); onInviteResolved?.(); }}
          onOpenSettings={tab => setDrawer(current => ({ open: true, tab: tab ?? current.tab }))} onConversationSelected={() => setPane('conversation')} />
      </div>
      <div className="teams-web-conversation">
        <button type="button" className="teams-web-back" onClick={() => setPane('list')}>{zh ? '‹ 会话列表' : '‹ Conversations'}</button>
        <TeamConversationView workspace={workspace} snapshot={snapshot} zh={zh} detailsOpen={drawer.open}
          onToggleDetails={() => setDrawer(current => ({ ...current, open: !current.open }))} />
      </div>
      {drawer.open ? <TeamDetailsDrawer workspace={workspace} snapshot={snapshot} zh={zh} tab={drawer.tab}
        onTabChange={tab => setDrawer(current => ({ ...current, tab }))} onClose={() => setDrawer(current => ({ ...current, open: false }))} /> : null}
    </div>
  );
}
