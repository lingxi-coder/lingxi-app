import { createHash, randomBytes } from 'node:crypto';
import { constants, closeSync, fstatSync, fsyncSync, lstatSync, mkdirSync, openSync, readSync, realpathSync, writeFileSync } from 'node:fs';
import { lstat, mkdir, open, readdir, unlink } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import type { TeamBotTarget } from './process';

export interface TeamModelRoute {
  model_provider: string; model_protocol: string; model_id: string; model_endpoint: string;
  credential_id: string; model_profile: Record<string, unknown>;
}
export interface TeamPersonaSnapshot extends TeamModelRoute { instructions: string }
export interface TeamModelSelection { route: TeamModelRoute; assertCurrent(): Promise<void> }

/** Native catalog descriptors contain one resolved static route, never owner routing. */
export function validateTeamModelRoute(value: unknown): asserts value is TeamModelRoute {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('INVALID_TEAM_SNAPSHOT');
  const route = value as TeamModelRoute;
  if (Object.keys(route).sort().join(',') !== 'credential_id,model_endpoint,model_id,model_profile,model_protocol,model_provider'
    || typeof route.model_provider !== 'string' || !route.model_provider || Buffer.byteLength(route.model_provider) > 512
    || /[\p{Cc}\p{White_Space}\/]/u.test(route.model_provider)
    || typeof route.model_protocol !== 'string' || !/^[a-z][a-z0-9_]{0,63}$/u.test(route.model_protocol)
    || typeof route.model_id !== 'string' || !route.model_id || route.model_id.length > 256 || /[\s\u0000-\u001f\u007f]/u.test(route.model_id)
    || typeof route.credential_id !== 'string' || !/^[a-z0-9][a-z0-9._-]{0,63}$/u.test(route.credential_id)
    || !route.model_profile || typeof route.model_profile !== 'object' || Array.isArray(route.model_profile)
    || Buffer.byteLength(JSON.stringify(route.model_profile)) > 64 * 1024
    || typeof route.model_endpoint !== 'string' || route.model_endpoint.length > 2048 || /\s/u.test(route.model_endpoint)) throw new Error('INVALID_TEAM_SNAPSHOT');
  let endpoint: URL;
  try { endpoint = new URL(route.model_endpoint); } catch { throw new Error('INVALID_TEAM_SNAPSHOT'); }
  const loopbackHttp = /^http:\/\/(?:127\.0\.0\.1|\[::1\])(?::[0-9]+)?(?:\/|$)/u.test(route.model_endpoint);
  if (endpoint.username || endpoint.password || route.model_endpoint.includes('?') || route.model_endpoint.includes('#')
    || (endpoint.protocol !== 'https:' && !loopbackHttp)) throw new Error('INVALID_TEAM_SNAPSHOT');
}
export interface PendingTeamTarget { version: 1; target: TeamBotTarget; snapshot: string; sequence: number }
export const digest = (value: string): string => createHash('sha256').update(value).digest('hex');

function privateDirectory(path: string): string {
  try { mkdirSync(path, { mode: 0o700 }); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
  const info = lstatSync(path);
  if (!info.isDirectory() || (info.mode & 0o077) !== 0 || (process.getuid && info.uid !== process.getuid())) throw new Error('UNSAFE_TEAM_STORAGE');
  return path;
}
function readPrivate(path: string, maxBytes: number): string {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const info = fstatSync(fd);
    if (!info.isFile() || info.nlink !== 1 || (info.mode & 0o077) !== 0 || info.size > maxBytes
      || (process.getuid && info.uid !== process.getuid())) throw new Error('UNSAFE_TEAM_STORAGE');
    const value = Buffer.alloc(maxBytes + 1), length = readSync(fd, value, 0, value.length, 0);
    if (length > maxBytes) throw new Error('UNSAFE_TEAM_STORAGE');
    return new TextDecoder('utf-8', { fatal: true }).decode(value.subarray(0, length));
  } finally { closeSync(fd); }
}
function createPrivate(path: string, value: string): void {
  const fd = openSync(path, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try { writeFileSync(fd, value); fsyncSync(fd); } finally { closeSync(fd); }
  const parent = openSync(dirname(path), constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  try { fsyncSync(parent); } finally { closeSync(parent); }
}
async function directory(path: string): Promise<string> {
  try { await mkdir(path, { mode: 0o700 }); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
  const info = await lstat(path);
  if (!info.isDirectory() || (info.mode & 0o077) !== 0 || (process.getuid && info.uid !== process.getuid())) throw new Error('UNSAFE_TEAM_STORAGE');
  return path;
}
async function read(path: string, maxBytes: number): Promise<string> {
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const info = await file.stat();
    if (!info.isFile() || info.nlink !== 1 || (info.mode & 0o077) !== 0 || info.size > maxBytes
      || (process.getuid && info.uid !== process.getuid())) throw new Error('UNSAFE_TEAM_STORAGE');
    const bytes = Buffer.alloc(maxBytes + 1);
    let length = 0;
    while (length < bytes.length) {
      const { bytesRead } = await file.read(bytes, length, bytes.length - length, length);
      if (!bytesRead) break;
      length += bytesRead;
    }
    if (length > maxBytes) throw new Error('UNSAFE_TEAM_STORAGE');
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, length));
  } finally { await file.close(); }
}
async function syncDirectory(path: string): Promise<void> {
  const file = await open(path, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  try { await file.sync(); } finally { await file.close(); }
}
async function create(path: string, value: string): Promise<void> {
  const file = await open(path, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try { await file.writeFile(value); await file.sync(); } finally { await file.close(); }
  await syncDirectory(dirname(path));
}
function pendingText(value: PendingTeamTarget, uid: string): string {
  const target = value?.target;
  if (!value || Object.keys(value).sort().join(',') !== 'sequence,snapshot,target,version' || value.version !== 1
    || !target || Object.keys(target).sort().join(',') !== 'botId,botKey,botRevision,conversationId,hostId,ownerGeneration,ownerUid,teamId,triggerId'
    || target.ownerUid !== uid || !/^[A-Za-z0-9_-]{1,128}$/u.test(uid)
    || !/^[A-Za-z0-9_-]{22}$/u.test(target.botId) || !/^[A-Za-z0-9_-]{22}$/u.test(target.hostId)
    || ![target.teamId, target.conversationId, target.triggerId].every(id => typeof id === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(id))
    || target.botKey !== `bot:${uid}:${target.hostId}:${target.botId}`
    || ![target.ownerGeneration, target.botRevision, value.sequence].every(number => Number.isSafeInteger(number) && number > 0)
    || typeof value.snapshot !== 'string' || !/^[a-f0-9]{64}$/u.test(value.snapshot)) throw new Error('UNSAFE_TEAM_STORAGE');
  return JSON.stringify(value);
}

/** Stable, account-specific roots beside Bots, never inside an ordinary Bot home. */
export class TeamPrivateStore {
  readonly root: string;
  readonly hostId: string;
  private operation: Promise<unknown> = Promise.resolve();
  private serial<T>(run: () => Promise<T>): Promise<T> {
    const result = this.operation.then(run);
    this.operation = result.catch(() => undefined);
    return result;
  }
  constructor(userData: string) {
    const actual = realpathSync.native(userData);
    this.root = privateDirectory(join(actual, 'team-bot-host'));
    const hostPath = join(this.root, 'host-id');
    try { createPrivate(hostPath, randomBytes(16).toString('base64url')); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    this.hostId = readPrivate(hostPath, 22);
    if (!/^[A-Za-z0-9_-]{22}$/u.test(this.hostId)) throw new Error('UNSAFE_TEAM_STORAGE');
  }
  async owner(uid: string): Promise<{ journal: string; runtime: string; snapshots: string; pending: string; home: string; tmp: string }> {
    if (!/^[A-Za-z0-9_-]{1,128}$/u.test(uid)) throw new Error('BOT_ACCOUNT_UNSUPPORTED');
    await directory(this.root);
    const root = await directory(join(this.root, digest(uid)));
    return { journal: await directory(join(root, 'journal')), runtime: await directory(join(root, 'runtime')),
      snapshots: await directory(join(root, 'snapshots')), pending: await directory(join(root, 'pending')),
      home: await directory(join(root, 'host-home')), tmp: await directory(join(root, 'host-tmp')) };
  }
  /** Durable discovery precedes spawning. No credential, attempt or mutable model state. */
  savePending(uid: string, value: PendingTeamTarget): Promise<void> { return this.serial(async () => {
    const text = pendingText(value, uid), path = join((await this.owner(uid)).pending, digest(JSON.stringify(value.target)));
    try { await create(path, text); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    if (await read(path, 4096) !== text) throw new Error('UNSAFE_TEAM_STORAGE');
  }); }
  pending(uid: string): Promise<PendingTeamTarget[]> { return this.serial(async () => {
    const root = (await this.owner(uid)).pending, names = await readdir(root);
    if (names.length > 1000) throw new Error('UNSAFE_TEAM_STORAGE');
    const values: PendingTeamTarget[] = [];
    for (const name of names) {
      if (!/^[a-f0-9]{64}$/u.test(name)) throw new Error('UNSAFE_TEAM_STORAGE');
      let text: string;
      try { text = await read(join(root, name), 4096); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue; throw error; }
      const value = JSON.parse(text) as PendingTeamTarget;
      if (pendingText(value, uid) !== text || digest(JSON.stringify(value.target)) !== name) throw new Error('UNSAFE_TEAM_STORAGE');
      values.push(value);
    }
    return values.sort((left, right) => left.sequence - right.sequence);
  }); }
  /** A concurrent settlement may have removed a previously listed entry. */
  hasPending(uid: string, value: PendingTeamTarget): Promise<boolean> { return this.serial(async () => {
    const text = pendingText(value, uid), path = join((await this.owner(uid)).pending, digest(JSON.stringify(value.target)));
    try {
      if (await read(path, 4096) !== text) throw new Error('UNSAFE_TEAM_STORAGE');
      return true;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false;
      throw error;
    }
  }); }
  /** Called only after an authenticated host settlement ACK or unstarted proof. */
  settlePending(uid: string, value: PendingTeamTarget): Promise<void> { return this.serial(async () => {
    const text = pendingText(value, uid), root = (await this.owner(uid)).pending, path = join(root, digest(JSON.stringify(value.target)));
    if (await read(path, 4096) !== text) throw new Error('UNSAFE_TEAM_STORAGE');
    await unlink(path);
    await syncDirectory(root);
  }); }
  saveSnapshot(uid: string, value: TeamPersonaSnapshot): Promise<string> { return this.serial(async () => {
    this.validateSnapshot(value);
    const text = JSON.stringify(value);
    if (Buffer.byteLength(text) > 192 * 1024) throw new Error('INVALID_TEAM_SNAPSHOT');
    const key = digest(text), path = join((await this.owner(uid)).snapshots, key);
    try { await create(path, text); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    if (await read(path, 192 * 1024) !== text) throw new Error('INVALID_TEAM_SNAPSHOT');
    return key;
  }); }
  snapshot(uid: string, key: string): Promise<TeamPersonaSnapshot> { return this.serial(async () => {
    if (!/^[a-f0-9]{64}$/u.test(key)) throw new Error('INVALID_TEAM_SNAPSHOT');
    const text = await read(join((await this.owner(uid)).snapshots, key), 192 * 1024);
    if (digest(text) !== key) throw new Error('INVALID_TEAM_SNAPSHOT');
    const value = JSON.parse(text) as TeamPersonaSnapshot;
    this.validateSnapshot(value);
    return value;
  }); }
  private validateSnapshot(value: TeamPersonaSnapshot): void {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('INVALID_TEAM_SNAPSHOT');
    const { instructions, ...route } = value;
    validateTeamModelRoute(route);
    if (typeof instructions !== 'string' || !instructions.trim() || instructions.includes('\0')
      || Buffer.byteLength(instructions) > 64 * 1024) throw new Error('INVALID_TEAM_SNAPSHOT');
  }
}
