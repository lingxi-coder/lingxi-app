import { useState, type ReactElement } from 'react';
import { friendsErrorText } from './errors.js';
import type { FriendsViewProps } from './FriendsSidebar';
import { Avatar } from './ui';

/** Blocked people (D12): the newest block first, with Unblock and no confirmation. Names are the ones kept when the block was made. */
export function BlockedList({ workspace, snapshot, zh, renderAvatar }: FriendsViewProps): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [busy, setBusy] = useState<string>();
  const [failed, setFailed] = useState<{ uid: string; code: string }>();
  const unblock = async (uid: string) => {
    setBusy(uid); setFailed(undefined);
    const code = await workspace.unblock(uid);
    setBusy(undefined);
    if (code) setFailed({ uid, code });
  };
  return <section className="tw-scope tw-friends-page" aria-labelledby="tw-friends-blocked">
    <h2 id="tw-friends-blocked">{t('Blocked people', '已屏蔽的人')}</h2>
    {snapshot.blocksLoading ? <p className="tw-hint" role="status">{t('Loading…', '正在加载…')}</p>
      : snapshot.blocksError ? <div className="tw-form">
        <p role="alert" className="tw-error">{t("Couldn't load your friends. Check your connection and try again.", '无法加载好友，请检查网络后重试。')}</p>
        <button type="button" onClick={workspace.retry}>{t('Retry', '重试')}</button>
      </div>
      : !snapshot.blocks.length ? <p className="tw-hint">{t("You haven't blocked anyone.", '你还没有屏蔽任何人。')}</p>
      : <ul className="tw-people" aria-label={t('Blocked people', '已屏蔽的人')}>
        {snapshot.blocks.map(block => {
          const name = block.displayName || t('Member', '成员');
          return <li key={block.uid}>
            <Avatar identity={{ kind: 'user', id: block.uid, name }} size={32} render={renderAvatar} />
            <div className="tw-person-text">
              <b>{name}</b>
              {block.handle ? <small>@{block.handle}</small> : null}
              {failed?.uid === block.uid ? <small role="alert" className="tw-error">{friendsErrorText(failed.code, zh)}</small> : null}
            </div>
            <div className="tw-person-actions"><button type="button" disabled={busy === block.uid} onClick={() => void unblock(block.uid)}>{t('Unblock', '解除屏蔽')}</button></div>
          </li>;
        })}
      </ul>}
    <p className="tw-hint">{t("Unblocking doesn't restore the friendship. Either of you can send a new request.", '解除屏蔽不会恢复好友关系，双方都可以重新发送请求。')}</p>
  </section>;
}
