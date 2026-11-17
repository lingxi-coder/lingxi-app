import { useState, type ReactElement } from 'react';
import { teamsErrorText } from './errors.js';
import { inviteCode } from './index.js';
import { Dialog, platformNotice } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';
import { useTeamsText } from './i18n.js';

interface DialogProps { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; locale: string; onClose(): void }

/** Name a new team. The team opens once the command is applied. */
export function CreateTeamDialog({ workspace, snapshot, locale, onClose }: DialogProps): ReactElement {
  const t = useTeamsText(locale);
  const [name, setName] = useState(''), [tried, setTried] = useState(false);
  const submit = async () => {
    if (!name.trim() || snapshot.busy) return;
    setTried(true);
    if (await workspace.execute({ action: 'create', teamId: crypto.randomUUID(), name: name.trim() })) onClose();
  };
  return <Dialog title={t('tw_sidebar_create_team')} onClose={onClose}>
    <form className="tw-form" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <label>{t('teams_name')}
        <input data-autofocus maxLength={80} required value={name} onChange={event => setName(event.target.value)} placeholder={t('teams_name')} autoComplete="off" />
      </label>
      <p className="tw-hint">{platformNotice(locale)}</p>
      {tried && snapshot.error ? <p role="alert" className="tw-error">{teamsErrorText(snapshot.error, locale)}</p> : null}
      <div className="tw-dialog-actions">
        <button type="button" onClick={onClose}>{t('common_cancel')}</button>
        <button type="submit" className="tw-primary" disabled={snapshot.busy || !name.trim()}>{t('teams_create')}</button>
      </div>
    </form>
  </Dialog>;
}

/** Review an invitation, then join. Opening a link never joins by itself. */
export function JoinTeamDialog({ workspace, snapshot, locale, initialCode = '', onClose, onJoined }: DialogProps & { initialCode?: string; onJoined?(): void }): ReactElement {
  const t = useTeamsText(locale);
  const [code, setCode] = useState(initialCode), [tried, setTried] = useState(false);
  const parsed = inviteCode(code);
  const submit = async () => {
    if (!parsed || snapshot.busy) return;
    setTried(true);
    if (await workspace.execute({ action: 'join', code: parsed, requestId: crypto.randomUUID() })) { onJoined?.(); onClose(); }
  };
  return <Dialog title={t('teams_join')} onClose={onClose}>
    <form className="tw-form" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <label>{t('teams_invite_input')}
        <input data-autofocus value={code} onChange={event => setCode(event.target.value)} maxLength={200} autoComplete="off" spellCheck={false}
          placeholder={t('teams_invite_input')} aria-invalid={code.trim() !== '' && !parsed} />
      </label>
      {code.trim() !== '' && !parsed ? <p className="tw-hint">{t('teams_invite_invalid')}</p> : null}
      <p className="tw-hint">{t('teams_join_disclosure')}</p>
      <p className="tw-hint">{platformNotice(locale)}</p>
      {tried && snapshot.error ? <p role="alert" className="tw-error">{teamsErrorText(snapshot.error, locale)}</p> : null}
      <div className="tw-dialog-actions">
        <button type="button" onClick={onClose}>{t('common_cancel')}</button>
        <button type="submit" className="tw-primary" disabled={snapshot.busy || !parsed}>{t('teams_join_confirm')}</button>
      </div>
    </form>
  </Dialog>;
}
