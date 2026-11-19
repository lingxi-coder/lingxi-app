import { FriendsMain } from '../../../../../packages/teams/FriendsMain';
import type { UseFriends } from '../../../../../packages/teams/useFriendsWorkspace';
import { tr } from '../i18n/locale';
import { FriendsSignIn } from './FriendsSignIn';
import { renderTeamAvatar } from './TeamAvatar';
import { renderTeamMarkdown } from './TeamsMain';
import type { InboxNoticeTarget } from '../../../../../packages/inbox/contracts';

/**
 * The Friends page: a friend's chat, Add friend, Profile settings or Blocked people, whichever the workspace shows. The sidebar
 * (`FriendsPanel`) is the other half; both read the one workspace the window holds. A separate chunk, like the Teams page.
 */
export function FriendsPage({ friends, locale, notificationTarget }: { friends: UseFriends; locale: string; notificationTarget?: InboxNoticeTarget }) {
  const { workspace, snapshot, signedIn } = friends;
  if (!signedIn) return <FriendsSignIn />;
  if (!workspace || !snapshot) return <p role="status" style={{ margin: 'auto' }}>{tr('friends_loading')}</p>;
  return <div className="friends-page"><FriendsMain shareLink workspace={workspace} snapshot={snapshot} locale={locale} notificationTarget={notificationTarget} renderAvatar={renderTeamAvatar} renderMarkdown={renderTeamMarkdown} /></div>;
}
