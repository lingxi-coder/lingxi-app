import { DESKTOP_BRIDGE_GLOBAL } from '../../shared/desktopBridge';
import { useMemo } from 'react';
import { useTeamsWorkspace, type UseTeams } from '../../../../../packages/teams/useTeamsWorkspace';
import type { TeamsWorkspaceOptions } from '../../../../../packages/teams/workspace';
import { useAccountState } from './useAccountState';
import { desktopTeamsApi, type HostBackground } from './desktopTeamsApi';
export { desktopTeamsApi, registerDesktopInboxReads, type HostBackground } from './desktopTeamsApi';

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
