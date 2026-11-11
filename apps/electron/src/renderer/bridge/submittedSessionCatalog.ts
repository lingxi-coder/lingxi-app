import type { SessionRowDto } from '@lingxi/bridge-client';
import type { BootstrapState, SessionRef } from './lingxi';

export interface SubmittedSession {
  projectPath: string;
  row: SessionRowDto;
}

/** Reconcile rare authoritative runtime replacements, including removals. */
export function reconcileSubmittedSessions(
  previous: ReadonlyMap<string, SubmittedSession>,
  states: ReadonlyMap<string, { submittedSession?: SubmittedSession }>,
): ReadonlyMap<string, SubmittedSession> {
  const next = new Map<string, SubmittedSession>();
  const prior = previous.entries();
  let unchanged = true;
  for (const [sessionId, state] of states) {
    if (state.submittedSession) {
      next.set(sessionId, state.submittedSession);
      const entry = prior.next().value;
      if (entry?.[0] !== sessionId || entry[1] !== state.submittedSession) unchanged = false;
    }
  }
  // Runtime insertion order owns sequential overlay precedence, even when an
  // older runtime submits its first prompt after a newer runtime does.
  return unchanged && prior.next().done ? previous : next;
}

/** Build the provisional sidebar row as soon as a first message is submitted. */
export function firstSubmittedSession(
  ref: SessionRef | undefined,
  text: string,
  saved: SessionRowDto | undefined,
): SubmittedSession | undefined {
  if (!ref || (saved && saved.message_count > 0)) return undefined;
  const mode = ref.mode ?? saved?.mode;
  if (!mode) return undefined;
  return {
    projectPath: ref.projectPath,
    row: {
      uuid: ref.sessionId,
      title: text.trim().replace(/\s+/g, ' ').slice(0, 120),
      modified_rfc3339: new Date().toISOString(),
      message_count: 1,
      mode,
      path: saved?.path ?? '',
    },
  };
}

/** Keep first-message rows visible until the saved catalog catches up. */
export function submittedSessionCatalogs(
  catalogs: BootstrapState['projectCatalogs'],
  submitted: Iterable<SubmittedSession>,
  archived: readonly SessionRef[],
): BootstrapState['projectCatalogs'] {
  const archivedByProject = new Map<string, Set<string>>();
  for (const ref of archived) {
    let ids = archivedByProject.get(ref.projectPath);
    if (!ids) archivedByProject.set(ref.projectPath, ids = new Set());
    ids.add(ref.sessionId);
  }
  const byProject = new Map<string, SessionRowDto[]>();
  for (const { projectPath, row } of submitted) {
    if (archivedByProject.get(projectPath)?.has(row.uuid)) continue;
    let rows = byProject.get(projectPath);
    if (!rows) byProject.set(projectPath, rows = []);
    rows.push(row);
  }
  let result = catalogs;
  for (const [projectPath, rows] of byProject) {
    const catalog = catalogs[projectPath];
    const savedById = new Map<string, SessionRowDto>();
    for (const saved of catalog?.sessions ?? []) {
      if (!savedById.has(saved.uuid)) savedById.set(saved.uuid, saved);
    }
    const pending = new Map<string, SessionRowDto>();
    for (const row of rows) {
      const saved = pending.get(row.uuid) ?? savedById.get(row.uuid);
      if (saved && saved.message_count > 0) continue;
      // Match sequential prepends: first positive row wins, later pending rows
      // precede earlier ones, and metadata-only duplicates move to the front.
      pending.delete(row.uuid);
      pending.set(row.uuid, { ...saved, ...row });
    }
    if (!pending.size) continue;
    if (result === catalogs) result = { ...catalogs };
    result[projectPath] = { ...catalog, sessions: [
      ...Array.from(pending.values()).reverse(),
      ...(catalog?.sessions ?? []).filter((session) => !pending.has(session.uuid)),
    ] };
  }
  return result;
}
