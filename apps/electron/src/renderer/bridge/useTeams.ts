import { DESKTOP_BRIDGE_GLOBAL } from '../../shared/desktopBridge';
import { useMemo } from 'react';
import type { DesktopTeamsApi, TeamsApi } from '../../../../../packages/teams';
import { useTeamsWorkspace, type UseTeams } from '../../../../../packages/teams/useTeamsWorkspace';
import type { TeamsWorkspaceOptions } from '../../../../../packages/teams/workspace';
import { useAccountState } from './useAccountState';

/** The renderer's `TeamsApi` over the preload bridge, for one account. */
export function desktopTeamsApi(desktop: DesktopTeamsApi, uid: string): TeamsApi {
  return {
    localBots: () => desktop.localBots(uid), bindLocalBot: input => desktop.bindLocalBot(uid, input),
    list: () => desktop.list(uid), detail: teamId => desktop.detail(uid, teamId), execute: command => desktop.execute(uid, command),
    watchMessages: (scope, conversationId, next) => desktop.watchMessages(uid, scope, conversationId, next),
    messages: command => desktop.messages(uid, command), onMessagesChanged: callback => desktop.onMessagesChanged(callback),
    onChanged: callback => desktop.onChanged(callback),
  };
}

/** The Teams workspace of the signed-in LingXi account on this Desktop. */
export function useTeams(options: TeamsWorkspaceOptions = {}): UseTeams {
  const { state } = useAccountState();
  const desktop = typeof window === 'undefined' ? undefined : window[DESKTOP_BRIDGE_GLOBAL]?.teams;
  const uid = state?.user?.uid;
  const api = useMemo(() => desktop && uid ? desktopTeamsApi(desktop, uid) : undefined, [desktop, uid]);
  return useTeamsWorkspace(api, uid, options);
}
