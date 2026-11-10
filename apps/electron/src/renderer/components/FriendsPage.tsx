import { FriendsMain } from '../../../../../packages/teams/FriendsMain';
import type { UseFriends } from '../../../../../packages/teams/useFriendsWorkspace';
import { tr } from '../i18n/locale';
import { FriendsSignIn } from './FriendsSignIn';
import { renderTeamMarkdown } from './TeamsMain';

/**
 * The Friends page: a friend's chat, Add friend, Profile settings or Blocked people, whichever the workspace shows. The sidebar
 * (`FriendsPanel`) is the other half; both read the one workspace the window holds. A separate chunk, like the Teams page.
 */
export function FriendsPage({ friends, zh, locale }: { friends: UseFriends; zh: boolean; locale: string }) {
  const { workspace, snapshot, signedIn } = friends;
  if (!signedIn) return <FriendsSignIn />;
  if (!workspace || !snapshot) return <p role="status" style={{ margin: 'auto' }}>{tr('friends_loading')}</p>;
  return <div className="friends-page"><FriendsMain workspace={workspace} snapshot={snapshot} zh={zh} locale={locale} renderMarkdown={renderTeamMarkdown} /></div>;
}
