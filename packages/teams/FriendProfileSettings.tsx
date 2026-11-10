import { useEffect, useState, type ReactElement } from 'react';
import { friendsErrorText } from './errors.js';
import { handleValid, NAME_MAX, nameValid, type FriendProfile } from './friends.js';
import type { ProfileChanges, FriendsWorkspace } from './friends-workspace.js';
import type { FriendsViewProps } from './FriendsSidebar';

/** Profile settings (D11): display name, username and who can find me. */
export function FriendProfileSettings({ workspace, snapshot, zh }: FriendsViewProps): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const { profile } = snapshot;
  // D2: every open of this page reads the profile again.
  useEffect(() => { void workspace.refreshProfile(); }, [workspace]);
  return <section className="tw-scope tw-friends-page" aria-labelledby="tw-friends-profile">
    <h2 id="tw-friends-profile">{t('Profile settings', '个人资料设置')}</h2>
    {profile ? <ProfileForm key={profile.uid} workspace={workspace} profile={profile} zh={zh} />
      : snapshot.profileLoading ? <p className="tw-hint" role="status">{t('Loading…', '正在加载…')}</p>
      : <div className="tw-form">
        <p role="alert" className="tw-error">{friendsErrorText(snapshot.profileError, zh)}</p>
        <button type="button" onClick={() => void workspace.refreshProfile()}>{t('Retry', '重试')}</button>
      </div>}
  </section>;
}

function ProfileForm({ workspace, profile, zh }: { workspace: FriendsWorkspace; profile: FriendProfile; zh: boolean }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
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
      {t('Display name', '显示名称')}
      <input type="text" required maxLength={NAME_MAX} value={name} onChange={event => setName(event.target.value)} />
    </label>
    <label>
      {t('Username', '用户名')}
      <input type="text" autoCapitalize="off" autoCorrect="off" spellCheck={false} placeholder={profile.handle ?? ''} value={handle} aria-invalid={!!handle && !handleValid(handle)}
        onChange={event => setHandle(event.target.value)} />
      <span className="tw-hint">{t('3–24 letters, numbers or underscores. Once set, it can be changed but not removed.', '3–24 位字母、数字或下划线。设置后可以修改，但不能移除。')}</span>
      {error === 'HANDLE_TAKEN' ? <span role="alert" className="tw-error">{friendsErrorText(error, zh)}</span> : null}
    </label>
    <label className="tw-check"><input type="checkbox" role="switch" checked={byHandle} onChange={event => setByHandle(event.target.checked)} />{t('Let people find me by username', '允许通过用户名找到我')}</label>
    <label className="tw-check"><input type="checkbox" role="switch" checked={byEmail} onChange={event => setByEmail(event.target.checked)} />{t('Let people find me by email', '允许通过邮箱找到我')}</label>
    <p className="tw-hint">{t('Only people who type your exact username or verified email can find you. Your email address is never shown.', '只有输入你完整用户名或已验证邮箱的人才能找到你，你的邮箱地址不会被显示。')}</p>
    {error && error !== 'HANDLE_TAKEN' ? <p role="alert" className="tw-error">{friendsErrorText(error, zh)}</p> : null}
    <div className="tw-dialog-actions"><button type="submit" className="tw-primary" disabled={!valid || !Object.keys(changes).length || busy}>{t('Save', '保存')}</button></div>
  </form>;
}
