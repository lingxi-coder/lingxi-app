import { useState, type ReactElement } from 'react';
import { ChatPageHeading } from './ChatTopbar';
import { friendsErrorText } from './errors.js';
import type { FriendsViewProps } from './FriendsSidebar';
import { Avatar } from './ui';
import { useTeamsText } from './i18n.js';

/** Blocked people (D12): the newest block first, with Unblock and no confirmation. Names are the ones kept when the block was made. */
export function BlockedList({ workspace, snapshot, locale, renderAvatar }: FriendsViewProps): ReactElement {
  const t = useTeamsText(locale);
  const [busy, setBusy] = useState<string>();
  const [failed, setFailed] = useState<{ uid: string; code: string }>();
  const unblock = async (uid: string) => {
    setBusy(uid); setFailed(undefined);
    const code = await workspace.unblock(uid);
    setBusy(undefined);
    if (code) setFailed({ uid, code });
  };
  return <section className="tw-scope tw-friends-page" aria-labelledby="tw-friends-blocked">
    <ChatPageHeading id="tw-friends-blocked" title={t('friends_blocked_title')} />
    {snapshot.blocksLoading ? <p className="tw-hint" role="status">{t('teams_loading')}</p>
      : snapshot.blocksError ? <div className="tw-form">
        <p role="alert" className="tw-error">{t('friends_load_error')}</p>
        <button type="button" onClick={workspace.retry}>{t('common_retry')}</button>
      </div>
      : !snapshot.blocks.length ? <div className="tw-empty">
        <h3>{t('friends_blocked_empty_title')}</h3>
        <p>{t('friends_blocked_empty_body')}</p>
      </div>
      : <ul className="tw-people" aria-label={t('friends_blocked_title')}>
        {snapshot.blocks.map(block => {
          const name = block.displayName || t('teams_member');
          return <li key={block.uid}>
            <Avatar identity={{ kind: 'user', id: block.uid, name }} size={40} render={renderAvatar} />
            <div className="tw-person-text">
              <b>{name}</b>
              {block.handle ? <small>@{block.handle}</small> : null}
              {failed?.uid === block.uid ? <small role="alert" className="tw-error">{friendsErrorText(failed.code, locale)}</small> : null}
            </div>
            <div className="tw-person-actions"><button type="button" disabled={busy === block.uid} onClick={() => void unblock(block.uid)}>{t('friends_unblock')}</button></div>
          </li>;
        })}
      </ul>}
    <p className="tw-hint">{t('friends_unblock_footer')}</p>
  </section>;
}
