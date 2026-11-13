import { DESKTOP_BRIDGE_GLOBAL } from '../../shared/desktopBridge';
import { useMemo } from 'react';
import type { DesktopTeamsApi, TeamsApi } from '../../../../../packages/teams';
import { useTeamsWorkspace, type UseTeams } from '../../../../../packages/teams/useTeamsWorkspace';
import type { TeamsWorkspaceOptions } from '../../../../../packages/teams/workspace';
import { useAccountState } from './useAccountState';

/** The two switches of the Bots page that keep this Mac serving, as the Teams pages change them. */
export interface HostBackground { setKeepAwake(enabled: boolean): Promise<unknown>; setOpenAtLogin(enabled: boolean): Promise<unknown>; openLoginItems(): Promise<unknown> }

/** The renderer's `TeamsApi` over the preload bridge, for one account. */
export function desktopTeamsApi(desktop: DesktopTeamsApi, uid: string, background?: HostBackground): TeamsApi {
  return {
    hostStatus: () => desktop.hostStatus(uid),
    ...(background ? {
      setHostBackground: async change => {
        if (change.openAtLogin !== undefined) await background.setOpenAtLogin(change.openAtLogin);
        if (change.keepAwake !== undefined) await background.setKeepAwake(change.keepAwake);
      },
      openLoginItems: async () => { await background.openLoginItems(); },
    } : {}),
    localBots: () => desktop.localBots(uid),
    selectSharedFiles: instanceId => desktop.selectSharedFiles(uid, instanceId),
    publishedKnowledge: (teamId, sharedBotId) => desktop.publishedKnowledge(uid, teamId, sharedBotId),
    publishLocalBot: input => desktop.publishLocalBot(uid, input),
    offerSharedBot: input => desktop.offerSharedBot(uid, input),
    acceptSharedBot: input => desktop.acceptSharedBot(uid, input),
    openBotMarket: () => desktop.openBotMarket(uid),
    list: () => desktop.list(uid), detail: teamId => desktop.detail(uid, teamId), execute: command => desktop.execute(uid, command),
    watchMessages: (scope, conversationId, next) => desktop.watchMessages(uid, scope, conversationId, next),
    messages: command => desktop.messages(uid, command), onMessagesChanged: callback => desktop.onMessagesChanged(callback),
    onChanged: callback => desktop.onChanged(callback),
  };
}

/** The Teams workspace of the signed-in LingXi account on this Desktop. */
export function useTeams(options: TeamsWorkspaceOptions = {}, setKeepAwake?: (enabled: boolean) => Promise<unknown>): UseTeams {
  const { state } = useAccountState();
  const bridge = typeof window === 'undefined' ? undefined : window[DESKTOP_BRIDGE_GLOBAL];
  const desktop = bridge?.teams, loginItem = bridge?.bots?.loginItem;
  const uid = state?.user?.uid;
  // The keep-awake switch is a setting the window holds, so the window's own writer is used; the login item is the system's and is asked directly.
  const background = useMemo<HostBackground | undefined>(() => setKeepAwake && loginItem
    ? { setKeepAwake, setOpenAtLogin: enabled => loginItem.set(enabled), openLoginItems: () => loginItem.openSettings() } : undefined, [setKeepAwake, loginItem]);
  const api = useMemo(() => desktop && uid ? desktopTeamsApi(desktop, uid, background) : undefined, [desktop, uid, background]);
  return useTeamsWorkspace(api, uid, options);
}
