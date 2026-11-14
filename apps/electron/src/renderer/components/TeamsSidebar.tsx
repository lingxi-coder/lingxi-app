import type { CSSProperties } from 'react';
import { TeamSidebar } from '../../../../../packages/teams/TeamSidebar';
import type { DrawerTab } from '../../../../../packages/teams/ui';
import type { UseTeams } from '../../../../../packages/teams/useTeamsWorkspace';
import { tr } from '../i18n/locale';
import { renderTeamAvatar } from './TeamAvatar';
import { noteStyle, TeamsSignIn } from './TeamsSignIn';

export interface TeamsSidebarProps {
  teams: UseTeams;
  zh: boolean;
  /** What the sidebar's search box holds, trimmed and in lower case. */
  query: string;
  /** An invitation from a link, to be reviewed in the join dialog. */
  initialInvite: string | undefined;
  onInviteHandled(): void;
  /** Show the Teams page. Pressing anything in the sidebar brings it forward, since the page can be showing something else (Apps, say) while the mode is Teams. */
  onShowTeams(): void;
  /** Open the details panel of the team, on a tab when one is asked for. */
  onOpenSettings(tab?: DrawerTab): void;
  /** Show Bot services in the Teams page, and go back to the Team when a conversation is chosen. */
  onOpenServices(): void;
  onCloseServices(): void;
}

/** The sidebar in Teams mode: the workspace's own sidebar, or a way to sign in when there is no account to open it for. */
export function TeamsSidebar({ teams, zh, query, initialInvite, onInviteHandled, onShowTeams, onOpenSettings, onOpenServices, onCloseServices }: TeamsSidebarProps) {
  const { workspace, snapshot, signedIn } = teams;
  if (!signedIn) return <nav className="no-drag teams-sidebar desktop-sidebar-list" aria-label={tr('teams_title')} style={listStyle}><TeamsSignIn compact /></nav>;
  if (!workspace || !snapshot) return <div className="no-drag teams-sidebar" style={listStyle}><p role="status" style={noteStyle}>{tr('desktop_teams_loading')}</p></div>;
  return (
    // The capture sees the press before the control does, so the page is already the Teams page when the control opens a dialog or the panel.
    <div className="no-drag teams-sidebar" style={hostStyle} onClickCapture={onShowTeams}>
      <TeamSidebar workspace={workspace} snapshot={snapshot} zh={zh} query={query} renderAvatar={renderTeamAvatar}
        initialInvite={initialInvite} onInviteHandled={onInviteHandled} onOpenSettings={onOpenSettings}
        onOpenServices={workspace.api.services && workspace.api.serviceMessages ? onOpenServices : undefined} onConversationSelected={onCloseServices} />
    </div>
  );
}

const listStyle: CSSProperties = { flex: 1, minHeight: 0, overflowY: 'auto', padding: '0 10px 12px' };
const hostStyle: CSSProperties = { flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' };

/** The places in the sidebar that can take the keyboard focus, best first: the open conversation, a row, the team switcher, any button. */
const FOCUS_TARGETS = ['[data-row][aria-current="page"]', '[data-row][tabindex="0"]', '.tw-switcher', 'button'];

/** Put the keyboard focus in the Teams sidebar, where a person who came to it with a key can carry on. Answers whether there was anywhere to put it. */
export function focusTeamsSidebar(root: ParentNode = document): boolean {
  const sidebar = root.querySelector('.teams-sidebar');
  for (const selector of FOCUS_TARGETS) {
    const target = sidebar?.querySelector<HTMLElement>(selector);
    if (target) { target.focus({ preventScroll: true }); target.scrollIntoView?.({ block: 'nearest' }); return true; }
  }
  return false;
}

/** `focusTeamsSidebar` once the sidebar is on screen: it appears with the next render when the mode has only just changed. */
export function focusTeamsSidebarSoon(frames = 12): void {
  const attempt = (left: number) => window.requestAnimationFrame(() => { if (!focusTeamsSidebar() && left > 1) attempt(left - 1); });
  attempt(frames);
}
