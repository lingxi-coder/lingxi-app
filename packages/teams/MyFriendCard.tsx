import { useEffect, useMemo, useState, type ReactElement } from 'react';
import qrcode from 'qrcode-generator';
import { ChatPageHeading } from './ChatTopbar';
import { friendsErrorText } from './errors.js';
import type { FriendsViewProps } from './FriendsSidebar';
import { Avatar, Dialog, Icon } from './ui';
import { useTeamsText } from './i18n.js';

export function MyFriendCard({ workspace, snapshot, locale, renderAvatar }: FriendsViewProps): ReactElement {
  const t = useTeamsText(locale);
  const { profile } = snapshot;
  useEffect(() => { void workspace.refreshProfile(); }, [workspace]);
  return <section className="tw-scope tw-friends-page" aria-labelledby="tw-friends-me">
    <ChatPageHeading id="tw-friends-me" title={t('friends_my_card')} />
    {profile ? <div className="tw-friends-card tw-friend-identity">
      <Avatar identity={{ kind: 'user', id: profile.uid, name: profile.displayName }} size={56} render={renderAvatar} />
      <div className="tw-person-text"><b>{profile.displayName}</b>{profile.handle ? <small>@{profile.handle}</small> : null}</div>
    </div> : null}
    <MyLink workspace={workspace} snapshot={snapshot} locale={locale} />
    <div className="tw-friend-settings">
      <button type="button" onClick={() => workspace.show('profile')}>{t('friends_profile_title')}<Icon name="chevronRight" size={16} /></button>
      <button type="button" onClick={() => workspace.show('blocked')}>{t('friends_blocked_title')}<Icon name="chevronRight" size={16} /></button>
    </div>
  </section>;
}

/** Middle-truncated on one line; the whole link stays selectable and copied. */
function LinkText({ link }: { link: string }): ReactElement {
  const cut = Math.max(8, link.length - 12);
  return <span className="tw-link-text" title={link}><span className="tw-link-head">{link.slice(0, cut)}</span><span className="tw-link-tail">{link.slice(cut)}</span></span>;
}

/** Invitation link and QR code share the same lifecycle. */
function MyLink({ workspace, snapshot, locale }: Pick<FriendsViewProps, 'workspace' | 'snapshot' | 'locale'>): ReactElement {
  const t = useTeamsText(locale);
  const { profile } = snapshot, code = profile?.friendCode;
  const [copied, setCopied] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [ask, setAsk] = useState<'reset' | 'off'>();
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);
  useEffect(() => { setCopied(false); }, [code?.link]);
  const run = async (action: () => Promise<string>) => {
    setBusy(true); setError('');
    const failed = await action();
    setBusy(false); setError(failed); setAsk(undefined);
  };
  const qr = useMemo(() => {
    if (!code?.link) return undefined;
    const image = qrcode(0, 'M');
    image.addData(code.link);
    image.make();
    return image.createDataURL(6, 24);
  }, [code?.link]);
  const share = typeof navigator !== 'undefined' && typeof navigator.share === 'function';
  return <section className="tw-friends-card" aria-labelledby="tw-friends-link">
    <h3 id="tw-friends-link">{t('friends_link_title')}</h3>
    {snapshot.profileLoading && !profile ? <p className="tw-hint" role="status">{t('teams_loading')}</p>
      : !profile ? <div className="tw-form">
        <p role="alert" className="tw-error">{friendsErrorText(snapshot.profileError, locale)}</p>
        <button type="button" onClick={() => void workspace.refreshProfile()}>{t('common_retry')}</button>
      </div>
      : code ? <>
        {qr ? <img className="tw-friend-qr" src={qr} alt={t('friends_link_qr')} /> : null}
        <LinkText link={code.link} />
        <div className="tw-lookup-actions">
          <button type="button" className="tw-primary" onClick={() => void navigator.clipboard?.writeText(code.link).then(() => setCopied(true), () => undefined)}>{t('friends_link_copy')}</button>
          {share ? <button type="button" onClick={() => void navigator.share({ text: `${t('friends_link_share_message')} ${code.link}` }).catch(() => undefined)}>{t('friends_link_share')}</button> : null}
          {copied ? <span className="tw-hint" role="status">{t('friends_link_copied')}</span> : null}
        </div>
        <div className="tw-lookup-actions">
          <button type="button" disabled={busy} onClick={() => setAsk('reset')}>{t('friends_link_reset')}</button>
          <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => setAsk('off')}>{t('friends_link_off')}</button>
        </div>
      </> : <div className="tw-lookup-actions">
        <span className="tw-hint">{t('friends_link_is_off')}</span>
        <button type="button" className="tw-primary" disabled={busy} onClick={() => void run(workspace.rotateCode)}>{t('friends_link_create')}</button>
      </div>}
    {error ? <p role="alert" className="tw-error">{friendsErrorText(error, locale)}</p> : null}
    <p className="tw-hint">{t('friends_link_disclosure')}</p>
    {ask ? <Dialog role="alertdialog" title={ask === 'reset' ? t('friends_link_reset_title') : t('friends_link_off_title')}
      description={ask === 'reset' ? t('friends_link_reset_body') : t('friends_link_off_body')}
      onClose={() => setAsk(undefined)}>
      <div className="tw-form">
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setAsk(undefined)}>{t('common_cancel')}</button>
          <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void run(ask === 'reset' ? workspace.rotateCode : workspace.revokeCode)}>{ask === 'reset' ? t('friends_link_reset') : t('friends_link_off')}</button>
        </div>
      </div>
    </Dialog> : null}
  </section>;
}
