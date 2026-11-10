import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { FriendsApi } from './friends.js';
import { FriendsWorkspace, type FriendsSnapshot, type FriendsWorkspaceOptions } from './friends-workspace.js';
import type { TeamsApi } from './index.js';

export interface UseFriends {
  /** Commands for the views (`FriendsSidebar`, `FriendsMain`). Absent until there is an account to open one for. */
  workspace: FriendsWorkspace | undefined;
  /** What the views render. Absent exactly when `workspace` is. */
  snapshot: FriendsSnapshot | undefined;
  /** An API and a signed-in account exist; otherwise the page should ask the person to sign in. */
  signedIn: boolean;
}

const noSubscription = (): (() => void) => () => undefined;
const noSnapshot = (): FriendsSnapshot | undefined => undefined;

/**
 * The Friends workspace for an account, opened with the account and closed (both listeners and the open chat released) when
 * the account or an api changes, on sign-out, or when the component unmounts. `options` are read when it is opened.
 * `teams` is the same account's Teams API, which carries the chat's messages.
 */
export function useFriendsWorkspace(api: FriendsApi | undefined, teams: TeamsApi | undefined, uid: string | undefined, options: FriendsWorkspaceOptions = {}): UseFriends {
  const [workspace, setWorkspace] = useState<FriendsWorkspace>();
  const opening = useRef(options);
  opening.current = options;
  useEffect(() => {
    if (!api || !teams || !uid) { setWorkspace(undefined); return; }
    const opened = new FriendsWorkspace(api, teams, uid, opening.current);
    setWorkspace(opened);
    return () => { opened.close(); setWorkspace(current => current === opened ? undefined : current); };
  }, [api, teams, uid]);
  const snapshot = useSyncExternalStore<FriendsSnapshot | undefined>(workspace ? workspace.subscribe : noSubscription, workspace ? workspace.getSnapshot : noSnapshot);
  return { workspace, snapshot, signedIn: !!api && !!teams && !!uid };
}
