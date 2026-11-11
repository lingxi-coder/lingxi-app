import { DESKTOP_BRIDGE_GLOBAL } from '../../shared/desktopBridge';
import { useMemo } from 'react';
import type { DesktopFriendsApi, FriendsApi } from '../../../../../packages/teams/friends';
import { useFriendsWorkspace, type UseFriends } from '../../../../../packages/teams/useFriendsWorkspace';
import { useAccountState } from './useAccountState';
import { desktopTeamsApi } from './useTeams';

/** The renderer's `FriendsApi` over the preload bridge, for one account. */
export function desktopFriendsApi(desktop: DesktopFriendsApi, uid: string): FriendsApi {
  return {
    profile: () => desktop.profile(uid),
    execute: command => desktop.execute(uid, command),
    watchCards: (next, failed) => desktop.watchCards(uid, next, failed),
    watchBlocks: (next, failed) => desktop.watchBlocks(uid, next, failed),
    watchOffers: (next, failed) => desktop.watchOffers(uid, next, failed),
    readProfile: friendUid => desktop.readProfile(uid, friendUid),
  };
}

/** The Friends workspace of the signed-in LingXi account on this Desktop. A friend's chat is carried by the same account's Teams bridge. */
export function useFriends(): UseFriends {
  const { state } = useAccountState();
  const bridge = typeof window === 'undefined' ? undefined : window[DESKTOP_BRIDGE_GLOBAL];
  const uid = state?.user?.uid;
  const api = useMemo(() => bridge?.friends && uid ? desktopFriendsApi(bridge.friends, uid) : undefined, [bridge, uid]);
  const teams = useMemo(() => bridge?.teams && uid ? desktopTeamsApi(bridge.teams, uid) : undefined, [bridge, uid]);
  return useFriendsWorkspace(api, teams, uid);
}
