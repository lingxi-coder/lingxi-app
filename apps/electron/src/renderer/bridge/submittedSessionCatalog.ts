import type { SessionRowDto } from '@lingxi/bridge-client';
import type { BootstrapState, SessionRef } from './lingxi';

export interface SubmittedSession {
  projectPath: string;
  row: SessionRowDto;
}

/** Build the provisional sidebar row as soon as a first message is submitted. */
export function firstSubmittedSession(
  ref: SessionRef | undefined,
  text: string,
  saved: SessionRowDto | undefined,
): SubmittedSession | undefined {
  if (!ref || (saved && saved.message_count > 0)) return undefined;
  return {
    projectPath: ref.projectPath,
    row: {
      uuid: ref.sessionId,
      title: text.trim().replace(/\s+/g, ' ').slice(0, 120),
      modified_rfc3339: new Date().toISOString(),
      message_count: 1,
      mode: saved?.mode ?? 'code',
      path: saved?.path ?? '',
    },
  };
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
