import type { ReactElement } from 'react';
import { AddFriendView } from './AddFriendView';
import { BlockedList } from './BlockedList';
import { FriendChatView, type FriendChatViewProps } from './FriendChatView';
import { FriendProfileSettings } from './FriendProfileSettings';

/**
 * The main area next to {@link FriendsSidebar}: the chat or the page the workspace is showing (D1). With nothing open it is the empty
 * state (U9): "No friends yet" while there are none, "Friends" otherwise, and the way to add one either way.
 */
export function FriendsMain(props: FriendChatViewProps): ReactElement {
  const { workspace, snapshot, zh } = props;
  const t = (en: string, cn: string) => zh ? cn : en;
  switch (snapshot.page) {
    case 'chat': return <FriendChatView {...props} />;
    case 'add': return <AddFriendView {...props} />;
    case 'profile': return <FriendProfileSettings {...props} />;
    case 'blocked': return <BlockedList {...props} />;
    default: return <section className="tw-scope tw-conv tw-conv-empty">
      <div className="tw-empty">
        <h2>{snapshot.cards.length || snapshot.offers.length ? t('Friends', '好友') : t('No friends yet', '还没有好友')}</h2>
        <p>{t('Add someone by @username, email or friend link to chat one-to-one.', '通过 @用户名、邮箱或好友链接添加好友，即可一对一聊天。')}</p>
        <div className="tw-empty-actions"><button type="button" className="tw-primary" onClick={() => workspace.show('add')}>{t('Add friend', '添加好友')}</button></div>
      </div>
    </section>;
  }
}
