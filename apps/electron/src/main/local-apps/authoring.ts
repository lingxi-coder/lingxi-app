import { promises as fs } from 'node:fs';
import type { ProjectSessionCatalog } from '../session-catalog.js';
import type { LiveDraftSession } from '../sessionRuntimeManager.js';
import type { AuthoringSessionSource } from './authoringSessionIntents.js';
import { nativeTr } from '../../shared/nativeLocale.js';

export interface DesktopAuthoringSessionRef { projectPath: string; sessionId: string }
export interface ResolvedDesktopAuthoring {
  sourceDirectory: string;
  projectRef: { kind: 'desktopPath'; path: string };
  authoringSessionId: string;
}

/** Resolve a session locator through the trusted Desktop catalog, then prove its cwd is the selected Project. */
export async function resolveDesktopAuthoringBinding(
  catalog: Pick<ProjectSessionCatalog, 'find'> & Partial<Pick<ProjectSessionCatalog, 'select'>>,
  session: DesktopAuthoringSessionRef,
  liveDraft?: LiveDraftSession,
  selected?: AuthoringSessionSource,
): Promise<ResolvedDesktopAuthoring> {
  if (selected && !catalog.select) throw new Error('The Local App session source cannot be verified');
  const source = selected ? await catalog.select!(session.projectPath, session.sessionId, selected)
    : await catalog.find(session.projectPath, session.sessionId);
  if (selected && (!source || source.path !== selected.transcriptPath)) throw new Error('The Local App session source changed');
  const projectPath = await fs.realpath(session.projectPath);
  if (source) {
    if (source.uuid !== session.sessionId || !source.workspace_path) {
      throw new Error('Choose a session with a saved workspace');
    }
    if (source.mode !== 'code') throw new Error(nativeTr('local_app_development_requires_code'));
    const actualPath = await fs.realpath(source.workspace_path);
    if (actualPath !== projectPath) throw new Error('The Local App source must be the dedicated Project cwd');
    if (!(await fs.stat(actualPath)).isDirectory()) throw new Error('The selected session workspace is not a directory');
    return { sourceDirectory: actualPath, projectRef: { kind: 'desktopPath', path: actualPath }, authoringSessionId: source.uuid };
  }

  // New sessions have a trusted native runtime before the first prompt
  // writes their transcript/header to the persisted catalog. Use its exact
  // identity and engine cwd only when the catalog has no row at all.
  if (!liveDraft || liveDraft.sessionId !== session.sessionId) {
    throw new Error('Choose a session with a saved workspace');
  }
  if (liveDraft.mode !== 'code') throw new Error(nativeTr('local_app_development_requires_code'));
  const [runtimeProjectPath, actualPath] = await Promise.all([
    fs.realpath(liveDraft.projectPath), fs.realpath(liveDraft.engineCwd),
  ]);
  if (runtimeProjectPath !== projectPath || actualPath !== projectPath) {
    throw new Error('The Local App source must be the dedicated Project cwd');
  }
  if (!(await fs.stat(actualPath)).isDirectory()) throw new Error('The selected session workspace is not a directory');
  return { sourceDirectory: actualPath, projectRef: { kind: 'desktopPath', path: actualPath }, authoringSessionId: liveDraft.sessionId };
}
