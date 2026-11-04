import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { TeamsApi } from './index.js';
import { TeamsWorkspace, type TeamsSnapshot, type TeamsWorkspaceOptions } from './workspace.js';

export interface UseTeams {
  /** Commands for the views (`TeamSidebar`, `TeamConversationView`, `TeamDetailsDrawer`). Absent until there is an account to open one for. */
  workspace: TeamsWorkspace | undefined;
  /** What the views render. Absent exactly when `workspace` is. */
  snapshot: TeamsSnapshot | undefined;
  /** A Teams API and a signed-in account exist; otherwise the page should ask the person to sign in. */
  signedIn: boolean;
}

const noSubscription = (): (() => void) => () => undefined;
const noSnapshot = (): TeamsSnapshot | undefined => undefined;

/**
 * The Teams workspace for an account's `TeamsApi`, opened with the account and closed (every listener,
 * timer and controller released) when the account or api changes or the component unmounts.
 * `options` are read when the workspace is opened. Desktop and Web both host the views through this.
 */
export function useTeamsWorkspace(api: TeamsApi | undefined, uid: string | undefined, options: TeamsWorkspaceOptions = {}): UseTeams {
  const [workspace, setWorkspace] = useState<TeamsWorkspace>();
  const opening = useRef(options);
  opening.current = options;
  useEffect(() => {
    if (!api || !uid) { setWorkspace(undefined); return; }
    const opened = new TeamsWorkspace(api, uid, opening.current);
    setWorkspace(opened);
    return () => { opened.close(); setWorkspace(current => current === opened ? undefined : current); };
  }, [api, uid]);
  const snapshot = useSyncExternalStore<TeamsSnapshot | undefined>(workspace ? workspace.subscribe : noSubscription, workspace ? workspace.getSnapshot : noSnapshot);
  return { workspace, snapshot, signedIn: !!api && !!uid };
}
