import type { SessionRowDto } from '@lingxi/bridge-client';
import type { BootstrapState, SessionRef } from './lingxi';

export interface SubmittedSession {
  projectPath: string;
  row: SessionRowDto;
}

/** Keep first-message rows visible until the saved catalog catches up. */
export function submittedSessionCatalogs(
  catalogs: BootstrapState['projectCatalogs'],
  submitted: readonly SubmittedSession[],
  archived: readonly SessionRef[],
): BootstrapState['projectCatalogs'] {
  const result = { ...catalogs };
  for (const { projectPath, row } of submitted) {
    if (archived.some((ref) => ref.projectPath === projectPath && ref.sessionId === row.uuid)) continue;
    const catalog = result[projectPath];
    const saved = catalog?.sessions.find((session) => session.uuid === row.uuid);
    if (saved && saved.message_count > 0) continue;
    result[projectPath] = {
      ...catalog,
      sessions: [{ ...saved, ...row }, ...(catalog?.sessions ?? []).filter((session) => session.uuid !== row.uuid)],
    };
  }
  return result;
}
