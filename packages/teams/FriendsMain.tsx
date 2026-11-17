import type { ReactElement } from 'react';
import { AddFriendView } from './AddFriendView';
import { BlockedList } from './BlockedList';
import { FriendChatView, type FriendChatViewProps } from './FriendChatView';
import { FriendProfileSettings } from './FriendProfileSettings';
import { useTeamsText } from './i18n.js';

/**
 * The main area next to {@link FriendsSidebar}: the chat or the page the workspace is showing (D1). With nothing open it is the empty
 * state (U9): "No friends yet" while there are none, "Friends" otherwise, and the way to add one either way.
 */
export function FriendsMain(props: FriendChatViewProps): ReactElement {
  const { workspace, snapshot, locale } = props;
  const t = useTeamsText(locale);
  switch (snapshot.page) {
    case 'chat': return <FriendChatView {...props} />;
    case 'add': return <AddFriendView {...props} />;
    case 'profile': return <FriendProfileSettings {...props} />;
    case 'blocked': return <BlockedList {...props} />;
    default: return <section className="tw-scope tw-conv tw-conv-empty">
      <div className="tw-empty">
        <h2>{snapshot.cards.length || snapshot.offers.length ? t('friends_title') : t('friends_empty_title')}</h2>
        <p>{t('friends_empty_body')}</p>
        <div className="tw-empty-actions"><button type="button" className="tw-primary" onClick={() => workspace.show('add')}>{t('friends_add')}</button></div>
      </div>
    </section>;
  }
}
