import { useEffect, useState, type ReactElement } from 'react';
import { ChatPageHeading } from './ChatTopbar';
import { friendsErrorText } from './errors.js';
import { handleValid, NAME_MAX, nameValid, type FriendProfile } from './friends.js';
import type { ProfileChanges, FriendsWorkspace } from './friends-workspace.js';
import type { FriendsViewProps } from './FriendsSidebar';
import { useTeamsText } from './i18n.js';

/** Profile settings (D11): display name, username and who can find me. */
export function FriendProfileSettings({ workspace, snapshot, locale }: FriendsViewProps): ReactElement {
  const t = useTeamsText(locale);
  const { profile } = snapshot;
  // D2: every open of this page reads the profile again.
  useEffect(() => { void workspace.refreshProfile(); }, [workspace]);
  return <section className="tw-scope tw-friends-page" aria-labelledby="tw-friends-profile">
    <ChatPageHeading id="tw-friends-profile" title={t('friends_profile_title')} />
    {profile ? <ProfileForm key={profile.uid} workspace={workspace} profile={profile} locale={locale} />
      : snapshot.profileLoading ? <p className="tw-hint" role="status">{t('teams_loading')}</p>
      : <div className="tw-form">
        <p role="alert" className="tw-error">{friendsErrorText(snapshot.profileError, locale)}</p>
        <button type="button" onClick={() => void workspace.refreshProfile()}>{t('common_retry')}</button>
      </div>}
  </section>;
}

function ProfileForm({ workspace, profile, locale }: { workspace: FriendsWorkspace; profile: FriendProfile; locale: string }): ReactElement {
  const t = useTeamsText(locale);
  const [name, setName] = useState(profile.displayName), [handle, setHandle] = useState(profile.handle ?? '');
  const [byHandle, setByHandle] = useState(profile.findByHandle), [byEmail, setByEmail] = useState(profile.findByEmail);
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  // Only what changed is sent. An empty username means "leave as is": once set it cannot be removed.
  const changes: ProfileChanges = {
    ...(name.trim() !== profile.displayName ? { displayName: name.trim() } : {}),
    ...(handle && handle !== profile.handle ? { handle } : {}),
    ...(byHandle !== profile.findByHandle ? { findByHandle: byHandle } : {}),
    ...(byEmail !== profile.findByEmail ? { findByEmail: byEmail } : {}),
  };
  const valid = nameValid(name) && (!handle || handleValid(handle));
  const save = async () => {
    setBusy(true); setError('');
    const code = await workspace.saveProfile(changes);
    setBusy(false); setError(code);
  };
  return <form className="tw-form" onSubmit={event => { event.preventDefault(); if (valid && Object.keys(changes).length && !busy) void save(); }}>
    <label>
      {t('friends_profile_name')}
      <input type="text" required maxLength={NAME_MAX} value={name} onChange={event => setName(event.target.value)} />
    </label>
    <label>
      {t('friends_profile_handle')}
      <input type="text" autoCapitalize="off" autoCorrect="off" spellCheck={false} placeholder={profile.handle ?? ''} value={handle} aria-invalid={!!handle && !handleValid(handle)}
        onChange={event => setHandle(event.target.value)} />
      <span className="tw-hint">{t('friends_profile_handle_hint')}</span>
      {error === 'HANDLE_TAKEN' ? <span role="alert" className="tw-error">{friendsErrorText(error, locale)}</span> : null}
    </label>
    <label className="tw-check"><input type="checkbox" role="switch" checked={byHandle} onChange={event => setByHandle(event.target.checked)} />{t('friends_profile_find_handle')}</label>
    <label className="tw-check"><input type="checkbox" role="switch" checked={byEmail} onChange={event => setByEmail(event.target.checked)} />{t('friends_profile_find_email')}</label>
    <p className="tw-hint">{t('friends_profile_find_footer')}</p>
    {error && error !== 'HANDLE_TAKEN' ? <p role="alert" className="tw-error">{friendsErrorText(error, locale)}</p> : null}
    <div className="tw-dialog-actions"><button type="submit" className="tw-primary" disabled={!valid || !Object.keys(changes).length || busy}>{t('common_save')}</button></div>
  </form>;
}
