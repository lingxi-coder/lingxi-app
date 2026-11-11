import { useEffect, useRef } from 'react';
import type { TeamsApi } from '../../../../packages/teams';
import '../../../../packages/teams/styles';
import { FriendLinkDialog } from '../../../../packages/teams/friend-dialogs';
import { badgeText, type FriendsApi } from '../../../../packages/teams/friends';
import { FriendsMain } from '../../../../packages/teams/FriendsMain';
import { FriendsSidebar } from '../../../../packages/teams/FriendsSidebar';
import { useFriendsWorkspace } from '../../../../packages/teams/useFriendsWorkspace';
import '../teams/teams-web.css';

interface LayoutProps {
  zh: boolean;
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
export function FriendsWorkspaceView({ friends, teams, uid, zh, route, chat, code, onCodeHandled, onBadge }: LayoutProps & { friends: FriendsApi; teams: TeamsApi; uid: string }) {
  const { workspace, snapshot } = useFriendsWorkspace(friends, teams, uid);
  const start = useRef({ route, chat });
  useEffect(() => {
    if (!workspace) return;
    if (start.current.chat) workspace.openChat(start.current.chat); else if (start.current.route === '/friends/add') workspace.show('add');
  }, [workspace]);
  const badge = snapshot ? badgeText(snapshot.badge) : null;
  useEffect(() => { onBadge(badge); return () => onBadge(null); }, [badge, onBadge]);
  if (!workspace || !snapshot) return <p role="status" className="teams-web-note">{zh ? '正在加载…' : 'Loading…'}</p>;
  return (
    <div className="teams-web-body" data-pane={snapshot.page === 'home' ? 'list' : 'conversation'}>
      <div className="teams-web-sidebar"><FriendsSidebar workspace={workspace} snapshot={snapshot} zh={zh} /></div>
      <div className="teams-web-conversation">
        <button type="button" className="teams-web-back" onClick={() => workspace.show('home')}>{zh ? '‹ 好友' : '‹ Friends'}</button>
        <FriendsMain workspace={workspace} snapshot={snapshot} zh={zh} />
      </div>
      {code ? <FriendLinkDialog workspace={workspace} zh={zh} code={code} onClose={onCodeHandled} /> : null}
    </div>
  );
}
