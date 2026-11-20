import { useEffect, useRef } from 'react';
import { Markdown } from '../remote/markdown';
import type { TeamsApi } from '../../../../packages/teams';
import '../../../../packages/teams/styles';
import { FriendLinkDialog } from '../../../../packages/teams/friend-dialogs';
import { badgeText, type FriendsApi } from '../../../../packages/teams/friends';
import { FriendsMain } from '../../../../packages/teams/FriendsMain';
import { FriendsSidebar } from '../../../../packages/teams/FriendsSidebar';
import { ChatTopbarControl } from '../../../../packages/teams/ChatTopbar';
import { useTeamsText } from '../../../../packages/teams/i18n';
import type { RenderMarkdown } from '../../../../packages/teams/ui';
import { useFriendsWorkspace } from '../../../../packages/teams/useFriendsWorkspace';
import { renderTeamAvatar } from '../teams/teamAvatar';
import '../teams/teams-web.css';

/** A Bot's messages in a friend chat use the Markdown renderer of Web's own chat, as in a Team. */
const renderMarkdown: RenderMarkdown = text => <Markdown text={text} />;

import type {InboxNoticeTarget} from '../../../../packages/inbox/contracts';
interface LayoutProps {
  notificationTarget?:InboxNoticeTarget;
  locale: string;
  /** `/friends/add` opens the Add friend page. */
  route: '/friends' | '/friends/add';
  /** A friend whose chat to open (`/friends?chat=`), once. */
  chat?: string;
  /** A friend link from the address bar or a kept intent: looked up and confirmed in a dialog, never redeemed on open (D8). */
  code: string;
  onCodeHandled(): void;
  /** The unread badge of D14 for the page header and the document title; null at 0. */
  onBadge(text: string | null): void;
}

/** The Friends workspace of a signed-in account, opened with it and closed when the account or the page goes (D19). */
export function FriendsWorkspaceView({ notificationTarget,friends, teams, uid, locale, route, chat, code, onCodeHandled, onBadge }: LayoutProps & { friends: FriendsApi; teams: TeamsApi; uid: string }) {
  const t = useTeamsText(locale);
  const { workspace, snapshot } = useFriendsWorkspace(friends, teams, uid);
  const start = useRef({ route, chat });
  useEffect(() => {
    if (!workspace) return;
    if (start.current.chat) workspace.openChat(start.current.chat); else if (start.current.route === '/friends/add') workspace.show('add');
  }, [workspace]);
  const badge = snapshot ? badgeText(snapshot.badge) : null;
  useEffect(() => { onBadge(badge); return () => onBadge(null); }, [badge, onBadge]);
  if (!workspace || !snapshot) return <p role="status" className="teams-web-note">{t('teams_loading')}</p>;
  return (
    <div className="teams-web-body" data-pane={snapshot.page === 'home' ? 'list' : 'conversation'}>
      <div className="teams-web-sidebar"><FriendsSidebar workspace={workspace} snapshot={snapshot} locale={locale} /></div>
      <div className="teams-web-conversation">
        {snapshot.page !== 'home' ? <ChatTopbarControl><button type="button" className="teams-web-back" onClick={() => workspace.show('home')}>{t('tw_back_friends')}</button></ChatTopbarControl> : null}
        <FriendsMain notificationTarget={notificationTarget} workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar} renderMarkdown={renderMarkdown} />
      </div>
      {code ? <FriendLinkDialog workspace={workspace} locale={locale} code={code} onClose={onCodeHandled} /> : null}
    </div>
  );
}
