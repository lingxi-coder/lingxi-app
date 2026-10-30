import { TeamsPanel } from '../../../../../packages/teams/TeamsPanel';
import { useAccountState } from '../bridge/useAccountState';
import { useMemo } from 'react';
import type { TeamsApi } from '../../../../../packages/teams';

export function Teams({ zh, initialInvite, onInviteHandled }: { zh: boolean; initialInvite?: string; onInviteHandled: () => void }) {
  const { state } = useAccountState();
  const desktop = window.lingxi?.teams, uid = state?.user?.uid;
  const api = useMemo<TeamsApi | undefined>(() => desktop && uid ? {
    localBots: () => desktop.localBots(uid), bindLocalBot: input => desktop.bindLocalBot(uid, input),
    list: () => desktop.list(uid), detail: teamId => desktop.detail(uid, teamId), execute: command => desktop.execute(uid, command),
    messages: command => desktop.messages(uid, command), onMessagesChanged: callback => desktop.onMessagesChanged(callback),
    onChanged: callback => desktop.onChanged(callback),
  } : undefined, [desktop, uid]);
  return api && uid ? <TeamsPanel key={uid} api={api} uid={uid} zh={zh} initialInvite={initialInvite} onInviteHandled={onInviteHandled} />
    : <p role="status">{zh ? '请登录灵犀账号以使用团队。' : 'Sign in to your LingXi account to use Teams.'}</p>;
}
