import { randomUUID } from 'node:crypto';
import { rename, rm, writeFile } from 'node:fs/promises';
import { isAbsolute } from 'node:path';

const actorId = /^[A-Za-z0-9_-]{1,64}$/u;

// The adapter is the only writer. The pinned service reads each complete snapshot
// from its private listener. A principal has at most one admitted operation.
export async function createDataActorProjection(path) {
  if (!isAbsolute(path)) throw new Error('Data actor file must use an absolute path');
  const active = new Map();
  const pending = new Map();
  let flush = Promise.resolve();

  function writeSnapshot() {
    const content = JSON.stringify(Object.fromEntries(active));
    const result = flush.then(async () => {
      const temporary = path + '.' + randomUUID() + '.tmp';
      try {
        await writeFile(temporary, content, { mode: 0o600, flag: 'wx' });
        await rename(temporary, path);
      } finally {
        await rm(temporary, { force: true });
      }
    });
    flush = result.catch(() => {});
    return result;
  }

  await writeSnapshot();
  return async function projectActor(uid, scope, authorize, run, recipients = [], agent) {
    const actors = [{ uid, generation: scope.generation, agent }, ...recipients];
    const ids = new Set(actors.map(actor => actor.uid));
    if (ids.size !== actors.length || actors.some(actor => !actorId.test(actor.uid))) {
      throw new Error('Unsupported Data actor identity');
    }
    if (scope.kind === 'personal' && recipients.length) throw new Error('Recipients require a team scope');
    const waits = [...ids].map(id => pending.get(id)?.catch(() => {}));
    const next = Promise.all(waits).then(async () => {
      await authorize();
      for (const actor of actors) {
        active.set(actor.uid, { id: actor.uid, teams: scope.kind === 'team' ? { [scope.teamId]: actor.generation } : {},
          ...(actor.agent ? { agent: actor.agent } : {}) });
      }
      try {
        await writeSnapshot();
        return await run();
      } finally {
        for (const id of ids) active.delete(id);
        await writeSnapshot();
      }
    });
    for (const id of ids) pending.set(id, next);
    try { return await next; }
    finally { for (const id of ids) if (pending.get(id) === next) pending.delete(id); }
  };
}
