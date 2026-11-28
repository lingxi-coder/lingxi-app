import { useRef, useState } from 'react';
import { teamsErrorText } from './errors';
import { useTeamsText } from './i18n';
import type { TeamProfile, TeamProfileChanges, TeamSummary } from './index';
import { TeamMark } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace';

const emptyProfile: TeamProfile = { version: 0, description: '', avatar: null };
const avatarTypes = ['image/png', 'image/jpeg', 'image/webp'];
const avatarAccept = [...avatarTypes, '.png', '.jpg', '.jpeg', '.webp'].join(',');
const avatarEdge = 96;

type ProfileFields = Required<TeamProfileChanges>;
type ProfileDraft = { version: number; base: ProfileFields; values: TeamProfileChanges };

const normalize = (value: string | null) => value?.trim() ?? null;

function wasEdited(draft: ProfileDraft, key: keyof ProfileFields): boolean {
  const value = draft.values[key];
  return value !== undefined && normalize(value) !== draft.base[key];
}

async function thumbnail(file: File): Promise<string> {
  if (!file.size || file.size > 20 * 1024 * 1024
    || (file.type ? !avatarTypes.includes(file.type) : !/\.(png|jpe?g|webp)$/iu.test(file.name))) throw new Error('TEAM_AVATAR_INVALID');
  const image = await createImageBitmap(file);
  try {
    const { width, height } = image;
    const edge = Math.min(width, height);
    const canvas = document.createElement('canvas');
    canvas.width = canvas.height = avatarEdge;
    const context = canvas.getContext('2d');
    if (!edge || !context) throw new Error('TEAM_AVATAR_INVALID');
    context.drawImage(image, (width - edge) / 2, (height - edge) / 2, edge, edge, 0, 0, avatarEdge, avatarEdge);
    const png = canvas.toDataURL('image/png').split(',')[1];
    if (!png || png.length > 64 * 1024) throw new Error('TEAM_AVATAR_INVALID');
    return png;
  } finally {
    image.close();
  }
}

export function TeamProfileSettings({ workspace, snapshot, team, locale }: {
  workspace: TeamsWorkspace; snapshot: TeamsSnapshot; team: TeamSummary; locale: string;
}) {
  const t = useTeamsText(locale);
  const detail = snapshot.detail?.team.id === team.id && snapshot.detail.team.generation === team.generation ? snapshot.detail : null;
  const profile = detail?.profile ?? emptyProfile;
  const savedName = detail?.team.name ?? team.name;
  const editable = team.role !== 'member';
  const [draft, setDraft] = useState<ProfileDraft>();
  const [error, setError] = useState('');
  const [avatarError, setAvatarError] = useState(false);
  const [prepared, setPrepared] = useState(true);
  const [reload, setReload] = useState(false);
  const [saved, setSaved] = useState(false);
  const picker = useRef<HTMLInputElement>(null);
  const saving = snapshot.busy || snapshot.offline || !detail;
  const base: ProfileFields = { name: savedName, description: profile.description, avatar: profile.avatar };
  const field = <Key extends keyof ProfileFields>(key: Key): ProfileFields[Key] => {
    const value = draft?.values[key];
    // Keep the input's raw text while typing. A reverted field follows live changes.
    return draft && value !== undefined && (wasEdited(draft, key) || base[key] === draft.base[key]) ? value : base[key];
  };
  const name = field('name'), description = field('description'), avatar = field('avatar');
  const changes: TeamProfileChanges = {};
  if (draft && wasEdited(draft, 'name') && name.trim() !== base.name) changes.name = name.trim();
  if (draft && wasEdited(draft, 'description') && description.trim() !== base.description) changes.description = description.trim();
  if (draft && wasEdited(draft, 'avatar') && avatar !== base.avatar) changes.avatar = avatar;
  const dirty = Object.keys(changes).length > 0;
  const choose = <Key extends keyof ProfileFields>(key: Key, value: ProfileFields[Key]) => {
    setSaved(false);
    if (!dirty && draft && profile.version > draft.version) setError('');
    setDraft(current => {
      const pending = current && (current.version === profile.version || dirty)
        ? current : { version: profile.version, base, values: {} };
      const start = wasEdited(pending, key) ? pending.base[key] : base[key];
      return { ...pending, base: { ...pending.base, [key]: start }, values: { ...pending.values, [key]: value } };
    });
  };
  const conflict = !!draft && dirty && (profile.version > draft.version || error === 'TEAM_PROFILE_CHANGED');

  const pick = async (file: File) => {
    setPrepared(false);
    setAvatarError(false);
    try { choose('avatar', await thumbnail(file)); }
    catch { setAvatarError(true); }
    finally { setPrepared(true); }
  };
  const rebase = async () => {
    setReload(true);
    try {
      await workspace.refresh();
      const latest = workspace.getSnapshot().detail;
      if (latest?.team.id !== team.id || latest.team.generation !== team.generation || !draft) return;
      const version = latest.profile?.version ?? 0;
      if (version <= draft.version) return;
      const newProfile = latest.profile ?? emptyProfile;
      setDraft({
        version,
        base: { name: latest.team.name, description: newProfile.description, avatar: newProfile.avatar },
        values: changes,
      });
      setError('');
    } finally { setReload(false); }
  };
  const save = async () => {
    if (!editable || saving || !prepared || !draft || !dirty || !name.trim() || conflict) return;
    setError('');
    if (await workspace.execute({ action: 'updateProfile', teamId: team.id, generation: team.generation, version: draft.version, changes })) {
      setDraft(undefined);
      setSaved(true);
    } else {
      setError(workspace.getSnapshot().error || 'SERVICE_UNAVAILABLE');
    }
  };

  return <form className="tw-form" onSubmit={event => { event.preventDefault(); void save(); }}>
    <fieldset className="tw-settings-fields" disabled={snapshot.busy || reload || !prepared}>
      <div className="tw-team-avatar-setting">
        <TeamMark id={team.id} name={name} avatar={avatar} size={56} />
        <div>
          <strong>{t('tw_team_avatar')}</strong>
          {editable ? <div className="tw-team-avatar-actions" aria-busy={!prepared}>
            <input ref={picker} type="file" accept={avatarAccept} aria-label={t('tw_team_avatar')} hidden onChange={event => {
              const file = event.currentTarget.files?.[0];
              event.currentTarget.value = '';
              if (file) void pick(file);
            }} />
            <button type="button" disabled={saving || !prepared} onClick={() => picker.current?.click()}>{t(avatar ? 'tw_team_avatar_replace' : 'tw_team_avatar_choose')}</button>
            {avatar ? <button type="button" className="tw-link" disabled={saving || !prepared} onClick={() => {
              setAvatarError(false);
              choose('avatar', null);
            }}>{t('tw_team_avatar_remove')}</button> : null}
          </div> : null}
          {!prepared ? <p role="status">{t('tw_team_avatar_preparing')}</p> : null}
          {avatarError ? <p role="alert" className="tw-error">{t('tw_team_avatar_invalid')}</p> : null}
        </div>
      </div>
      <label>{t('teams_name')}<input value={name} maxLength={80} readOnly={!editable || !detail} required
        onChange={event => choose('name', event.currentTarget.value)} /></label>
      <label>{t('tw_channel_description')}<textarea value={description} maxLength={1000} rows={4} readOnly={!editable || !detail}
        onChange={event => choose('description', event.currentTarget.value)} /></label>
    </fieldset>
    {conflict ? <div className="tw-form">
      <p role="alert" className="tw-error">{t('tw_team_profile_changed')}</p>
      <button className="tw-link" type="button" disabled={saving || reload || !prepared} onClick={() => void rebase()}>{t('tw_team_profile_reload')}</button>
    </div> : error && error !== 'TEAM_PROFILE_CHANGED' ? <p role="alert" className="tw-error">{teamsErrorText(error, locale)}</p> : null}
    {saved ? <p role="status">{t('tw_team_profile_saved')}</p> : null}
    {editable ? <footer><button type="submit" className="tw-primary" disabled={saving || reload || !prepared || !dirty || !name.trim() || conflict}>{t('common_save')}</button></footer> : null}
  </form>;
}
