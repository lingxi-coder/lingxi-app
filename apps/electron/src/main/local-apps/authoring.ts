import { promises as fs } from 'node:fs';
import type { ProjectSessionCatalog } from '../session-catalog.js';
import type { LiveDraftSession } from '../sessionRuntimeManager.js';

export interface DesktopAuthoringSessionRef { projectPath: string; sessionId: string }
export interface ResolvedDesktopAuthoring {
  sourceDirectory: string;
  projectRef: { kind: 'desktopPath'; path: string };
  authoringSessionId: string;
}

/** Resolve a session locator through the trusted Desktop catalog, then prove its Code cwd is the selected Project. */
export async function resolveDesktopAuthoringBinding(
  catalog: Pick<ProjectSessionCatalog, 'find'>,
  session: DesktopAuthoringSessionRef,
  liveDraft?: LiveDraftSession,
): Promise<ResolvedDesktopAuthoring> {
  const source = await catalog.find(session.projectPath, session.sessionId);
  const projectPath = await fs.realpath(session.projectPath);
  if (source) {
    if (source.uuid !== session.sessionId || source.mode !== 'code' || !source.workspace_path) {
      throw new Error('Choose a Code session with a saved workspace');
    }
    const actualPath = await fs.realpath(source.workspace_path);
    if (actualPath !== projectPath) throw new Error('The Local App source must be the dedicated Code Project cwd');
    if (!(await fs.stat(actualPath)).isDirectory()) throw new Error('The selected Code session workspace is not a directory');
    return { sourceDirectory: actualPath, projectRef: { kind: 'desktopPath', path: actualPath }, authoringSessionId: source.uuid };
  }

  // New Code sessions have a trusted native runtime before the first prompt
  // writes their transcript/header to the persisted catalog. Use its exact
  // identity and engine cwd only when the catalog has no row at all.
  if (!liveDraft || liveDraft.mode !== 'code' || liveDraft.sessionId !== session.sessionId) {
    throw new Error('Choose a Code session with a saved workspace');
  }
  const [runtimeProjectPath, actualPath] = await Promise.all([
    fs.realpath(liveDraft.projectPath), fs.realpath(liveDraft.engineCwd),
  ]);
  if (runtimeProjectPath !== projectPath || actualPath !== projectPath) {
    throw new Error('The Local App source must be the dedicated Code Project cwd');
  }
  if (!(await fs.stat(actualPath)).isDirectory()) throw new Error('The selected Code session workspace is not a directory');
  return { sourceDirectory: actualPath, projectRef: { kind: 'desktopPath', path: actualPath }, authoringSessionId: liveDraft.sessionId };
}
