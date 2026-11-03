import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { DesktopTeamsApi, TeamsApi } from '../../../../../packages/teams';
import { TeamsWorkspace, type TeamsSnapshot, type TeamsWorkspaceOptions } from '../../../../../packages/teams/workspace';
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

export interface UseTeams {
  /** Commands for the views (`TeamSidebar`, `TeamConversationView`, `TeamDetailsDrawer`). Absent until there is an account to open one for. */
  workspace: TeamsWorkspace | undefined;
  /** What the views render. Absent exactly when `workspace` is. */
  snapshot: TeamsSnapshot | undefined;
  /** A Teams bridge and a signed-in account exist; otherwise the page should ask the person to sign in. */
  signedIn: boolean;
}

const noSubscription = (): (() => void) => () => undefined;
const noSnapshot = (): TeamsSnapshot | undefined => undefined;

/**
 * The Teams workspace for an account's `TeamsApi`, opened with the account and closed (every listener,
 * timer and controller released) when the account or api changes or the component unmounts.
 * `options` are read when the workspace is opened.
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

/** The Teams workspace of the signed-in LingXi account on this Desktop. */
export function useTeams(options: TeamsWorkspaceOptions = {}): UseTeams {
  const { state } = useAccountState();
  const desktop = typeof window === 'undefined' ? undefined : window.lingxi?.teams;
  const uid = state?.user?.uid;
  const api = useMemo(() => desktop && uid ? desktopTeamsApi(desktop, uid) : undefined, [desktop, uid]);
  return useTeamsWorkspace(api, uid, options);
}
