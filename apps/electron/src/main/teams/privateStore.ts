import { createHash, randomBytes } from 'node:crypto';
import { constants, closeSync, fstatSync, fsyncSync, lstatSync, mkdirSync, openSync, readSync, readdirSync, realpathSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { TeamBotTarget } from './process';

export interface TeamPersonaSnapshot { instructions: string; model_id: string; model_endpoint: string }
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
  constructor(userData: string) {
    const actual = realpathSync.native(userData);
    this.root = privateDirectory(join(actual, 'team-bot-host'));
    const hostPath = join(this.root, 'host-id');
    try { createPrivate(hostPath, randomBytes(16).toString('base64url')); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    this.hostId = readPrivate(hostPath, 22);
    if (!/^[A-Za-z0-9_-]{22}$/u.test(this.hostId)) throw new Error('UNSAFE_TEAM_STORAGE');
  }
  owner(uid: string): { journal: string; runtime: string; snapshots: string; pending: string; home: string; tmp: string } {
    if (!/^[A-Za-z0-9_-]{1,128}$/u.test(uid)) throw new Error('BOT_ACCOUNT_UNSUPPORTED');
    privateDirectory(this.root);
    const root = privateDirectory(join(this.root, digest(uid)));
    return { journal: privateDirectory(join(root, 'journal')), runtime: privateDirectory(join(root, 'runtime')),
      snapshots: privateDirectory(join(root, 'snapshots')), pending: privateDirectory(join(root, 'pending')),
      home: privateDirectory(join(root, 'host-home')), tmp: privateDirectory(join(root, 'host-tmp')) };
  }
  /** Durable discovery precedes spawning. No credential, attempt or mutable model state. */
  savePending(uid: string, value: PendingTeamTarget): void {
    const text = pendingText(value, uid), path = join(this.owner(uid).pending, digest(JSON.stringify(value.target)));
    try { createPrivate(path, text); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    if (readPrivate(path, 4096) !== text) throw new Error('UNSAFE_TEAM_STORAGE');
  }
  pending(uid: string): PendingTeamTarget[] {
    const root = this.owner(uid).pending, names = readdirSync(root);
    if (names.length > 1000) throw new Error('UNSAFE_TEAM_STORAGE');
    return names.map(name => {
      if (!/^[a-f0-9]{64}$/u.test(name)) throw new Error('UNSAFE_TEAM_STORAGE');
      const text = readPrivate(join(root, name), 4096), value = JSON.parse(text) as PendingTeamTarget;
      if (pendingText(value, uid) !== text || digest(JSON.stringify(value.target)) !== name) throw new Error('UNSAFE_TEAM_STORAGE');
      return value;
    }).sort((left, right) => left.sequence - right.sequence);
  }
  /** A concurrent settlement may have removed a previously listed entry. */
  hasPending(uid: string, value: PendingTeamTarget): boolean {
    const text = pendingText(value, uid), path = join(this.owner(uid).pending, digest(JSON.stringify(value.target)));
    try {
      if (readPrivate(path, 4096) !== text) throw new Error('UNSAFE_TEAM_STORAGE');
      return true;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false;
      throw error;
    }
  }
  /** Called only after an authenticated host settlement ACK or unstarted proof. */
  settlePending(uid: string, value: PendingTeamTarget): void {
    const text = pendingText(value, uid), root = this.owner(uid).pending, path = join(root, digest(JSON.stringify(value.target)));
    if (readPrivate(path, 4096) !== text) throw new Error('UNSAFE_TEAM_STORAGE');
    unlinkSync(path);
    const directory = openSync(root, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
    try { fsyncSync(directory); } finally { closeSync(directory); }
  }
  saveSnapshot(uid: string, value: TeamPersonaSnapshot): string {
    const text = JSON.stringify(value);
    if (Buffer.byteLength(text) > 192 * 1024) throw new Error('INVALID_TEAM_SNAPSHOT');
    const key = digest(text), path = join(this.owner(uid).snapshots, key);
    try { createPrivate(path, text); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    if (readPrivate(path, 192 * 1024) !== text) throw new Error('INVALID_TEAM_SNAPSHOT');
    return key;
  }
  snapshot(uid: string, key: string): TeamPersonaSnapshot {
    if (!/^[a-f0-9]{64}$/u.test(key)) throw new Error('INVALID_TEAM_SNAPSHOT');
    const text = readPrivate(join(this.owner(uid).snapshots, key), 192 * 1024);
    if (digest(text) !== key) throw new Error('INVALID_TEAM_SNAPSHOT');
    const value = JSON.parse(text) as TeamPersonaSnapshot;
    if (Object.keys(value).sort().join(',') !== 'instructions,model_endpoint,model_id' || typeof value.instructions !== 'string'
      || !value.instructions.trim() || value.instructions.includes('\0') || Buffer.byteLength(value.instructions) > 64 * 1024
      || typeof value.model_id !== 'string' || !/^[A-Za-z0-9._-]{1,256}$/u.test(value.model_id)
      || value.model_endpoint !== 'https://api.anthropic.com') throw new Error('INVALID_TEAM_SNAPSHOT');
    return value;
  }
}
