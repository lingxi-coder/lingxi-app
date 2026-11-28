import { useCallback, useEffect, useRef, useState } from 'react';
import { teamsErrorText } from './errors';
import { useTeamsText } from './i18n';
import type { TeamMember, TeamRemovalBot, TeamSummary } from './index';
import { Dialog } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace';

export function TeamMemberRemoval({ workspace, snapshot, team, member, locale, onClose }: {
  workspace: TeamsWorkspace; snapshot: TeamsSnapshot; team: TeamSummary; member: TeamMember; locale: string; onClose(): void;
}) {
  const t = useTeamsText(locale);
  const [bots, setBots] = useState<TeamRemovalBot[]>();
  const [pause, setPause] = useState<TeamRemovalBot>();
  const [checking, setChecking] = useState(true);
  const [error, setError] = useState('');
  const request = useRef(0);
  const confirmationBack = useRef<HTMLButtonElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const { id: teamId, generation: actorGeneration } = team;
  const { uid, generation } = member;
  const check = useCallback(async () => {
    const token = ++request.current;
    setChecking(true);
    try {
      const result = await workspace.api.execute({ action: 'removalBlockers', teamId, uid, generation });
      if (token !== request.current) return;
      if (!result.blockingBots) throw new Error('INVALID_INPUT');
      setBots(result.blockingBots);
    } catch (cause) {
      if (token !== request.current) return;
      setBots(undefined);
      setError(cause instanceof Error ? cause.message : 'SERVICE_UNAVAILABLE');
    } finally {
      if (token === request.current) setChecking(false);
    }
  }, [workspace, teamId, uid, generation]);
  useEffect(() => {
    void check();
    return () => { request.current++; };
  }, [check]);
  useEffect(() => { (pause ? confirmationBack : cancel).current?.focus(); }, [pause]);

  const remove = async () => {
    setError('');
    if (await workspace.execute({ action: 'remove', teamId, uid, generation })) {
      onClose();
      return;
    }
    const code = workspace.getSnapshot().error || 'SERVICE_UNAVAILABLE';
    setError(code);
    if (code === 'TEAM_MEMBER_HAS_BOTS') void check();
  };
  const stopBot = async (bot: TeamRemovalBot) => {
    setError('');
    const ok = await workspace.execute({ action: 'pauseRemovalBot', teamId, uid, generation, actorGeneration, botId: bot.id, revision: bot.revision });
    if (!ok) setError(workspace.getSnapshot().error || 'SERVICE_UNAVAILABLE');
    setPause(undefined);
    void check();
  };
  const busy = snapshot.busy || snapshot.offline || checking;

  return <Dialog title={t('teams_confirm_remove_title_fmt', member.displayName)} role="alertdialog"
    description={bots?.length === 0 ? t('teams_confirm_remove_body') : undefined} onClose={onClose}>
    {error ? <p role="alert" className="tw-error">{teamsErrorText(error, locale)}</p> : null}
    {checking ? <p role="status">{t('teams_loading')}</p> : null}
    {pause ? <div className="tw-form">
      <p>{t('tw_team_pause_remove_bot_fmt', pause.name)}</p>
      <div className="tw-dialog-actions">
        <button ref={confirmationBack} type="button" disabled={snapshot.busy} onClick={() => setPause(undefined)}>{t('remote_back')}</button>
        <button className="tw-danger-btn" type="button" disabled={busy} onClick={() => void stopBot(pause)}>{t('friends_share_bot_stop')}</button>
      </div>
    </div> : <>
      {bots?.length ? <>
        <p>{t('tw_team_remove_bots_first')}</p>
        <ul className="tw-channel-roster">{bots.map(bot => <li key={bot.id}>
          <span>{bot.name}</span>
          <button type="button" disabled={busy} onClick={() => setPause(bot)}>{t('friends_share_bot_stop')}</button>
        </li>)}</ul>
      </> : null}
      <div className="tw-dialog-actions">
        <button ref={cancel} type="button" data-autofocus onClick={onClose}>{t('common_cancel')}</button>
        {bots?.length === 0
          ? <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void remove()}>{t('teams_confirm_remove_button')}</button>
          : <button type="button" disabled={busy} onClick={() => { setError(''); void check(); }}>{t('tw_team_refresh_bot_list')}</button>}
      </div>
    </>}
  </Dialog>;
}
