import { collection, onSnapshot, query, where, doc, type Firestore } from 'firebase/firestore';
import { teamsTransport, type TeamsApi, type TeamCommand, type TeamDetail, type TeamResult, type TeamSummary } from '../../../../packages/teams';
import type { MessageCommand } from '../../../../packages/teams/messages';
import type { User } from 'firebase/auth';

export function webTeamsApi(user: User, db: Firestore): TeamsApi & { close(): void } {
  let closed = false;
  const request = teamsTransport('', async () => { const token = await user.getIdToken(); if (closed) throw new Error('INVALID_SESSION'); return token; });
  const callbacks = new Set<(event: { invalidated?: boolean }) => void>();
  const messageCallbacks = new Set<() => void>();
  const messageStops = new Map<string, () => void>();
  const stops = new Map<string, () => void>();
  const failed = new Map<string, number>();
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
    async messages<T>(command: MessageCommand): Promise<T> {
      const key = `${command.teamId}:${command.generation}`;
      if (!messageStops.has(key)) {
        messageStops.set(key, onSnapshot(query(collection(db, 'teams', command.teamId, 'conversations'), where('memberUids', 'array-contains', user.uid)),
          { includeMetadataChanges: true }, snapshot => { if (!closed && !snapshot.metadata.fromCache) for (const cb of messageCallbacks) cb(); },
          () => { if (!closed) for (const cb of messageCallbacks) cb(); }));
      }
      if (command.action === 'sync') {
        const readKey = `${key}:${command.conversationId}`;
        if (!messageStops.has(readKey)) messageStops.set(readKey, onSnapshot(doc(db, 'teams', command.teamId, 'conversations', command.conversationId, 'reads', user.uid),
          snapshot => { if (!closed && !snapshot.metadata.fromCache) for (const cb of messageCallbacks) cb(); }, () => { if (!closed) for (const cb of messageCallbacks) cb(); }));
      }
      return checked<T>('/messages', command);
    },
    onMessagesChanged(callback) { messageCallbacks.add(callback); return () => { messageCallbacks.delete(callback); if (!messageCallbacks.size) { for (const stop of messageStops.values()) stop(); messageStops.clear(); } }; },
    onChanged(callback) { callbacks.add(callback); return () => { callbacks.delete(callback); }; },
    close() { closed = true; for (const stop of stops.values()) stop(); stops.clear(); callbacks.clear(); for (const stop of messageStops.values()) stop(); messageStops.clear(); messageCallbacks.clear(); },
  };
}
