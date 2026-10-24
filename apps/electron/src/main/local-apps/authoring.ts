import { promises as fs } from 'node:fs';
import type { ProjectSessionCatalog } from '../session-catalog.js';

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
): Promise<ResolvedDesktopAuthoring> {
  const source = await catalog.find(session.projectPath, session.sessionId);
  if (!source || source.mode !== 'code' || !source.workspace_path) throw new Error('Choose a Code session with a saved workspace');
  const [actualPath, projectPath] = await Promise.all([fs.realpath(source.workspace_path), fs.realpath(session.projectPath)]);
  if (actualPath !== projectPath) throw new Error('The Local App source must be the dedicated Code Project cwd');
  if (!(await fs.stat(actualPath)).isDirectory()) throw new Error('The selected Code session workspace is not a directory');
  return { sourceDirectory: actualPath, projectRef: { kind: 'desktopPath', path: actualPath }, authoringSessionId: source.uuid };
}
