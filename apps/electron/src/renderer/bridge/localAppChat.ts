import type { DesktopLocalApp, LocalAppEditRequest } from '../../shared/localApps';
import type { ProjectCreationPurpose } from '../../shared/projectCreation';

type SessionRef = NonNullable<DesktopLocalApp['sourceSession']>;
export interface LocalAppDraft {
  id: string;
  name: string;
  nonce: number;
  target: SessionRef;
}
export interface LocalAppChatNavigation {
  hasProject(path: string): boolean;
  openSession(projectPath: string, sessionId: string): Promise<boolean | undefined>;
  addProject(purpose?: ProjectCreationPurpose): Promise<{ path?: string } | null | undefined | void>;
  newSession(projectPath: string): Promise<SessionRef | undefined | void>;
}

/** Navigate through the renderer bridge so its selected session and draft agree. */
export async function localAppChatTarget(
  app: LocalAppEditRequest | undefined,
  navigation: LocalAppChatNavigation,
  isCurrent: () => boolean,
): Promise<SessionRef | undefined> {
  if (!isCurrent()) return;
  const source = app?.sourceSession;
  let project: string | undefined;
  if (source && navigation.hasProject(source.projectPath)) {
    const opened = await navigation.openSession(source.projectPath, source.sessionId);
    // A failed or superseded navigation must never draft into an unrelated chat.
    if (!isCurrent()) return;
    if (opened) return source;
    // The application survives deletion of its authoring chat. Keep its project
    // but establish a new explicit target before adding any text.
    project = source.projectPath;
  }
  // New and imported apps, or a removed source project, need an explicit
  // authoring workspace. Never reuse an unrelated active repository.
  if (!project) {
    const selected = await navigation.addProject('local-app');
    if (!selected?.path || !isCurrent()) return;
    project = selected.path;
  }
  const failure = `Could not open the app authoring chat. The project is saved at ${project}. Choose that source folder to retry.`;
  let target: SessionRef | undefined | void;
  try { target = await navigation.newSession(project); }
  catch (cause) {
    if (!isCurrent()) return;
    throw Object.assign(new Error(`${failure}\n${cause instanceof Error ? cause.message : String(cause)}`), { cause });
  }
  if (!isCurrent()) return;
  if (!target) throw new Error(failure);
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
