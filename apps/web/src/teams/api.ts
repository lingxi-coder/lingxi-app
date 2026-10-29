import { collection, onSnapshot, type Firestore } from 'firebase/firestore';
import { teamsTransport, type TeamsApi, type TeamCommand, type TeamDetail, type TeamResult, type TeamSummary } from '../../../../packages/teams';
import type { User } from 'firebase/auth';

export function webTeamsApi(user: User, db: Firestore): TeamsApi & { close(): void } {
  const request = teamsTransport('', () => user.getIdToken());
  const callbacks = new Set<(event: { invalidated?: boolean }) => void>();
  const stops = new Map<string, () => void>();
  const failed = new Map<string, number>();
  let closed = false;
  const emit = (invalidated = false) => { if (!closed) for (const callback of callbacks) callback({ invalidated }); };
  const watch = (key: string, path: string[], generation = 0) => {
    if (stops.has(key)) return;
    if (failed.get(key) === generation) return;
    stops.set(key, onSnapshot(collection(db, path[0]!, ...path.slice(1)), { includeMetadataChanges: true },
      snapshot => emit(snapshot.metadata.fromCache), () => { stops.get(key)?.(); stops.delete(key); failed.set(key, generation); emit(true); }));
  };
  const checked = async <T>(path = '', body?: unknown): Promise<T> => {
    if (closed) throw new Error('INVALID_SESSION');
    const result = await request<T>(path, body);
    if (closed) throw new Error('INVALID_SESSION');
    return result;
  };
  return {
    async list() { watch('index', ['users', user.uid, 'teams']); return checked<{ teams: TeamSummary[] }>(); },
    async detail(teamId: string) {
      if (!/^[A-Za-z0-9_-]{8,64}$/u.test(teamId)) throw new Error('INVALID_INPUT');
      const current = await checked<TeamDetail>(`/${teamId}`);
      watch(teamId, ['teams', teamId, 'members'], current.team.generation); return current;
    },
    execute: (command: TeamCommand) => checked<TeamResult>('', command),
    onChanged(callback) { callbacks.add(callback); return () => { callbacks.delete(callback); }; },
    close() { closed = true; for (const stop of stops.values()) stop(); stops.clear(); callbacks.clear(); },
  };
}
