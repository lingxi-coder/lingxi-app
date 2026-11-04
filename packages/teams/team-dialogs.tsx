import { useState, type ReactElement } from 'react';
import { teamsErrorText } from './errors.js';
import { inviteCode } from './index.js';
import { Dialog, platformNotice } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';

interface DialogProps { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; zh: boolean; onClose(): void }

/** Name a new team. The team opens once the command is applied. */
export function CreateTeamDialog({ workspace, snapshot, zh, onClose }: DialogProps): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [name, setName] = useState(''), [tried, setTried] = useState(false);
  const submit = async () => {
    if (!name.trim() || snapshot.busy) return;
    setTried(true);
    if (await workspace.execute({ action: 'create', teamId: crypto.randomUUID(), name: name.trim() })) onClose();
  };
  return <Dialog title={t('Create a team', '创建团队')} onClose={onClose}>
    <form className="tw-form" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <label>{t('Team name', '团队名称')}
        <input data-autofocus maxLength={80} required value={name} onChange={event => setName(event.target.value)} placeholder={t('Team name', '团队名称')} autoComplete="off" />
      </label>
      <p className="tw-hint">{platformNotice(zh)}</p>
      {tried && snapshot.error ? <p role="alert" className="tw-error">{teamsErrorText(snapshot.error, zh)}</p> : null}
      <div className="tw-dialog-actions">
        <button type="button" onClick={onClose}>{t('Cancel', '取消')}</button>
        <button type="submit" className="tw-primary" disabled={snapshot.busy || !name.trim()}>{t('Create team', '创建团队')}</button>
      </div>
    </form>
  </Dialog>;
}

/** Review an invitation, then join. Opening a link never joins by itself. */
export function JoinTeamDialog({ workspace, snapshot, zh, initialCode = '', onClose, onJoined }: DialogProps & { initialCode?: string; onJoined?(): void }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [code, setCode] = useState(initialCode), [tried, setTried] = useState(false);
  const parsed = inviteCode(code);
  const submit = async () => {
    if (!parsed || snapshot.busy) return;
    setTried(true);
    if (await workspace.execute({ action: 'join', code: parsed, requestId: crypto.randomUUID() })) { onJoined?.(); onClose(); }
  };
  return <Dialog title={t('Join a team', '加入团队')} onClose={onClose}>
    <form className="tw-form" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <label>{t('Invite link or code', '邀请链接或邀请码')}
        <input data-autofocus value={code} onChange={event => setCode(event.target.value)} maxLength={200} autoComplete="off" spellCheck={false}
          placeholder={t('Invite link or code', '邀请链接或邀请码')} aria-invalid={code.trim() !== '' && !parsed} />
      </label>
      {code.trim() !== '' && !parsed ? <p className="tw-hint">{t('Paste a LingXi invite link or the 32-character code.', '请粘贴灵犀邀请链接或 32 位邀请码。')}</p> : null}
      <p className="tw-hint">{t('Join only teams you trust. Your name will be visible to their members.', '仅加入你信任的团队。加入后团队成员可见你的名称。')}</p>
      <p className="tw-hint">{platformNotice(zh)}</p>
      {tried && snapshot.error ? <p role="alert" className="tw-error">{teamsErrorText(snapshot.error, zh)}</p> : null}
      <div className="tw-dialog-actions">
        <button type="button" onClick={onClose}>{t('Cancel', '取消')}</button>
        <button type="submit" className="tw-primary" disabled={snapshot.busy || !parsed}>{t('Confirm and join', '确认加入')}</button>
      </div>
    </form>
  </Dialog>;
}
