import type { GitScope } from '../../shared/git';
/** Project navigation does not replace the open conversation. Git follows the selected project. */
export function selectedGitScope(project: string | undefined, session: GitScope | null | undefined, workspace?: string): GitScope | null {
  const projectPath = project ?? session?.projectPath ?? workspace;
  return projectPath ? { projectPath, sessionId: session?.projectPath === projectPath ? session.sessionId : '__draft__' } : null;
}
