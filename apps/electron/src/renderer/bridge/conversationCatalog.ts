import type { SessionRowDto } from '@lingxi/bridge-client';
import type { ConversationSpace, OwnedConversation } from '../../shared/conversationSpaces';

/** A catalog path is a workspace, not necessarily the conversation's project. */
export function sessionsInLogicalScope(
  catalogs: Record<string, { sessions: SessionRowDto[] }> | undefined,
  owners: readonly OwnedConversation[], mode: ConversationSpace, projectId: string | null,
  globalPaths: readonly string[],
): Array<{ projectPath: string; session: SessionRowDto }> {
  const byId = new Map(owners.map((owner) => [owner.sessionId.toLowerCase(), owner]));
  const result: Array<{ projectPath: string; session: SessionRowDto }> = [];
  const seen = new Set<string>();
  for (const [projectPath, catalog] of Object.entries(catalogs ?? {})) {
    for (const session of catalog.sessions) {
      const id = session.uuid.toLowerCase();
      if (seen.has(id) || session.mode !== mode) continue;
      const owner = byId.get(id);
      if (owner ? owner.mode !== mode || owner.projectId !== projectId || owner.projectPath !== projectPath
        : projectId === null ? !globalPaths.includes(projectPath) : projectPath !== projectId) continue;
      seen.add(id);
      result.push({ projectPath, session });
    }
  }
  return result;
}
