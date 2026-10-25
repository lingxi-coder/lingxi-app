import type { DesktopAuthoringRef, LocalAppEditRequest } from '../../shared/localApps';
import { isDesktopAuthoringRef } from '../../shared/localApps';
import type { SessionRef } from '../../shared/settings';
import type { ProjectCreationPurpose } from '../../shared/projectCreation';

export interface LocalAppDraft {
  id: string;
  name: string;
  nonce: number;
  target: SessionRef;
}
export interface LocalAppChatNavigation {
  hasProject(path: string): boolean;
  openSession(projectPath: string, sessionId: string): Promise<SessionRef | false | undefined>;
  addProject(purpose?: ProjectCreationPurpose): Promise<{ path?: string } | null | undefined | void>;
  newSession(projectPath: string): Promise<SessionRef | undefined | void>;
  bindAuthoring(appId: string, session: SessionRef): Promise<DesktopAuthoringRef>;
  prepareAuthoring(session: SessionRef): Promise<DesktopAuthoringRef>;
}

function assertAuthoringSession(binding: DesktopAuthoringRef, session: SessionRef): void {
  if (binding.sourceDirectory !== session.projectPath || binding.projectRef.kind !== 'desktopPath'
    || binding.projectRef.path !== session.projectPath || binding.authoringSessionId !== session.sessionId) {
    throw new Error('Desktop did not verify the requested Local App source Code session');
  }
}

/** Navigate to the host-verified Code workspace before creating the draft. */
export async function localAppChatTarget(
  app: LocalAppEditRequest | undefined,
  navigation: LocalAppChatNavigation,
  isCurrent: () => boolean,
): Promise<SessionRef | undefined> {
  if (!isCurrent()) return;
  const sourceDirectory = app?.authoring?.sourceDirectory;
  const authoring = isDesktopAuthoringRef(app?.authoring) ? app.authoring : undefined;
  let project = authoring?.projectRef.path ?? sourceDirectory;
  if (app?.authoringSourceMissing) {
    // The old source is gone. Let the user locate a replacement or explicitly
    // restore it, then bind the new Code session before creating a draft.
    const restored = await navigation.addProject('local-app');
    if (!restored?.path || !isCurrent()) return;
    project = restored.path;
  } else if (project && !navigation.hasProject(project)) {
    const restored = await navigation.addProject('local-app');
    if (!restored?.path || !isCurrent()) return;
    if (restored.path !== project) throw new Error(`Add the saved app workspace at ${project} before editing this application.`);
    project = restored.path;
  } else if (!project) {
    const selected = await navigation.addProject('local-app');
    if (!selected?.path || !isCurrent()) return;
    project = selected.path;
  }

  if (authoring && !app?.authoringSourceMissing && app?.authoringAction === 'resumeSession' && navigation.hasProject(project)) {
    const opened = await navigation.openSession(project, authoring.authoringSessionId);
    if (!isCurrent()) return;
    if (opened && opened.projectPath === project) return opened;
    throw new Error(`Could not resume the saved app authoring session ${authoring.authoringSessionId} in ${project}. Refresh the Apps library and try again.`);
  }

  // An unavailable legacy/recovery session is replaced inside its saved source
  // project. A chat session or an unrelated active repository is never used.
  const failure = `Could not open the app authoring chat. The project is saved at ${project}. Choose that source folder to retry.`;
  let target: SessionRef | undefined | void;
  try { target = await navigation.newSession(project); }
  catch (cause) {
    if (!isCurrent()) return;
    throw Object.assign(new Error(`${failure}\n${cause instanceof Error ? cause.message : String(cause)}`), { cause });
  }
  if (!isCurrent()) return;
  if (!target) throw new Error(failure);
  if (app?.id) {
    assertAuthoringSession(await navigation.bindAuthoring(app.id, target), target);
    if (!isCurrent()) return;
  } else {
    // The native Create flow must durably register this exact Code session
    // before the composer exposes its first Local App draft.
    assertAuthoringSession(await navigation.prepareAuthoring(target), target);
    if (!isCurrent()) return;
  }
  return target;
}

/** Discard a request once another settled chat wins, rather than reviving it later. */
export function localAppDraftDisposition(
  draft: LocalAppDraft,
  activeSession: SessionRef | null | undefined,
  loading: boolean,
): 'wait' | 'apply' | 'discard' {
  if (loading) return 'wait';
  return activeSession?.sessionId === draft.target.sessionId
    && activeSession.projectPath === draft.target.projectPath ? 'apply' : 'discard';
}
