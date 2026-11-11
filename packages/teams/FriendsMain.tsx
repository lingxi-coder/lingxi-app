import type { ReactElement } from 'react';
import { AddFriendView } from './AddFriendView';
import { BlockedList } from './BlockedList';
import { FriendChatView, type FriendChatViewProps } from './FriendChatView';
import { FriendProfileSettings } from './FriendProfileSettings';

/** The main area next to {@link FriendsSidebar}: the chat or the page the workspace is showing (D1). */
export function FriendsMain(props: FriendChatViewProps): ReactElement {
  const { snapshot, zh } = props;
  switch (snapshot.page) {
    case 'chat': return <FriendChatView {...props} />;
    case 'add': return <AddFriendView {...props} />;
    case 'profile': return <FriendProfileSettings {...props} />;
    case 'blocked': return <BlockedList {...props} />;
    default: return <section className="tw-scope tw-conv tw-conv-empty">
      <div className="tw-empty">
        <h2>{zh ? '好友' : 'Friends'}</h2>
        <p>{zh ? '通过 @用户名、邮箱或好友链接添加好友，即可一对一聊天。' : 'Add someone by @username, email or friend link to chat one-to-one.'}</p>
      </div>
    </section>;
  }
}
