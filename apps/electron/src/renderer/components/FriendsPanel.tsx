import type { CSSProperties } from 'react';
import { FriendLinkDialog } from '../../../../../packages/teams/friend-dialogs';
import { FriendsSidebar } from '../../../../../packages/teams/FriendsSidebar';
import type { UseFriends } from '../../../../../packages/teams/useFriendsWorkspace';
import { tr } from '../i18n/locale';
import { FriendsSignIn } from './FriendsSignIn';
import { noteStyle } from './TeamsSignIn';

export interface FriendsPanelProps {
  friends: UseFriends;
  zh: boolean;
  /** BCP 47 tag of the app's language, for name order and times (D5). */
  locale: string;
  /** A friend link that was opened: looked up here and confirmed in a dialog, never redeemed on open (D8). */
  initialCode: string | undefined;
  onCodeHandled(): void;
  /** Show the Friends page. Pressing anything in the sidebar brings it forward, since the page can be showing something else (Apps, say) while the mode is Friends. */
  onShowFriends(): void;
}

/** The sidebar in Friends mode: the Friends root, or a way to sign in when there is no account to open it for. */
export function FriendsPanel({ friends, zh, locale, initialCode, onCodeHandled, onShowFriends }: FriendsPanelProps) {
  const { workspace, snapshot, signedIn } = friends;
  if (!signedIn) return <nav className="no-drag friends-sidebar desktop-sidebar-list" aria-label={tr('friends_title')} style={listStyle}><FriendsSignIn compact /></nav>;
  if (!workspace || !snapshot) return <div className="no-drag friends-sidebar" style={listStyle}><p role="status" style={noteStyle}>{tr('friends_loading')}</p></div>;
  return (
    // The capture sees the press before the control does, so the page is already the Friends page when the control opens a chat or a dialog.
    <div className="no-drag friends-sidebar" style={hostStyle} onClickCapture={onShowFriends}>
      <FriendsSidebar workspace={workspace} snapshot={snapshot} zh={zh} locale={locale} />
      {initialCode ? <FriendLinkDialog workspace={workspace} zh={zh} code={initialCode} onClose={onCodeHandled} /> : null}
    </div>
  );
}

const listStyle: CSSProperties = { flex: 1, minHeight: 0, overflowY: 'auto', padding: '0 10px 12px' };
const hostStyle: CSSProperties = { flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' };
