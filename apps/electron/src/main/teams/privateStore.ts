import { createHash, randomBytes } from 'node:crypto';
import { constants, closeSync, fstatSync, fsyncSync, lstatSync, mkdirSync, openSync, readSync, realpathSync, writeFileSync } from 'node:fs';
import { link, lstat, mkdir, open, readdir, rename, rm, unlink } from 'node:fs/promises';
import type { TeamRunResult, BotChatScope, TeamTaskCheckpoint } from './process.js';
import type { TaskUsage } from '../../../../../packages/teams/execution.js';
import { dirname, join } from 'node:path';
import { withProviderAccountLock } from '../provider-accounts.js';

export interface TeamModelRoute {
  model_provider: string; model_protocol: string; model_id: string; model_endpoint: string;
  credential_id: string; model_profile: Record<string, unknown>;
}
export interface PublishedDocument { resourceId: string; revision: number; kind: 'persona' | 'file' | 'skill'; name: string; text: string; digest: string }
export interface TeamPersonaSnapshot extends TeamModelRoute { instructions: string; provider_account_profiles: Record<string, unknown>[]; documents: PublishedDocument[] }
export interface SharedBotBinding { schemaVersion: 1; ownerUid: string; teamId: string; generation: number; sharedBotId: string; deploymentId: string; instanceId: string; revision: number; snapshot: string; accountEpoch: number; publicationId: string; grantIds: string[]; sessionEpoch: number }
export interface TeamDataRef { path: string; sha256: string; bytes: number }
/** A publication, or (`action`) the take-over of an offered Bot, saved before it is sent so that a lost answer is asked for again with the same command. */
export interface SavedPublication { schemaVersion: 1; requestDigest: string; binding: SharedBotBinding; payload: Record<string, unknown>; action?: 'completeTransfer' }
/** A grant candidate is not authority. Keep the original RPC and SDK baseline until the task is delivered. */
export interface SavedGrantIntent {
  action: 'startTask' | 'continueTask'; phase: 'pending' | 'validated'; nonce: string; commandId: string; targetEpoch: number;
  payload: Record<string, unknown>; binding: SharedBotBinding; checkpoint?: TeamTaskCheckpoint;
  messageId: string; requester: { uid: string; generation: number };
  cursor?: TeamTaskCheckpoint['cursor'];
  /** Incremented durably before each RPC; a later false answer cannot retire an earlier unknown request. */
  rpcAttempts?: number;
}
/** One task this host holds or held a model permit for. It lives until the task's result is delivered or the task can no longer continue.
 * Replaced in place as the task moves on: a new attempt, a question, an answer. */
export interface SavedExecution { schemaVersion: 2; scope: BotChatScope; teamId: string; generation: number; taskId: string;
  grant?: SavedGrantIntent;
  privateData?: { source: string; requesterUid: string };
  /** End of the current attempt's active budget. */ deadlineAt: number; commandId: string;
  /** Host fields, the current permit and the input grants: the base of every host command for the task. */ payload: Record<string, unknown>;
  /** Attempt number of the current permit: 1 after startTask, then one more per granted continueTask. */ epoch: number;
  /** The request message and the source watermark the task was admitted with. */ messageSeq: number; lastChangeSeq: number;
  /** The admitted input batch. A continuing worker is given the same bytes, so it is kept as long as this record. */ batch: TeamDataRef;
  /** The Bot may ask its requester a question (the deployment publishes durable waits). */ ask: boolean;
  /** Nobody asked the Bot in a group or channel: it never asks the member a question back. */ unasked?: boolean;
  /** Nobody asked the Bot and its triage was unsure: the Bot may leave the message unanswered. Every attempt is offered the same. */ mayDecline?: boolean;
  /** A person spoke to the Bot in a group without asking it (a reply to its message, its name): it answers, unless the message plainly needs no answer. Only with `mayDecline`. */ addressed?: true;
  /** The names of the other Bots the message that asked this one asked too: given to the run as information, so that each Bot answers its own part. */ coMentioned?: string[];
  /** The Bot's maintainer reads its reply before it is posted. Decided once, when the task started, from the Bot's own setting: a later change of the setting does not reach a task that is running. */ hold?: true;
  /** The files of Chat messages in the batch: the model was given them, and a finished task says so. */ files?: { attachmentId: string; sha256: string }[];
  /** The Bot's shared list, offered to the worker of this task. Its writes go through the operation ledger. */ list?: { title: string; description: string; approval?: 'requester' };
  /** The other Bots of the Chat the worker may hand part of the request to: as the service named them when the attempt was granted, so every recovery of that attempt is offered the same. */
  delegates?: { id: string; name: string; description?: string }[];
  /** Model requests of the attempts so far, as the server counted them. */ dispatched: number;
  /** Attempts continued after an interruption. Bounded, so a task that keeps dying ends. */ recoveries: number;
  /** Usage of the earlier attempts that reported one. */ usage?: TaskUsage;
  /** The exact pauseTask request for a question the worker asked, saved before it is sent so every retry is identical. */ pause?: Record<string, unknown>;
  /** The server accepted the question: the task continues only through its answer. With `delegation` the wait is for another Bot's reply: `expiresAt` is the time it may wait for it, after which the host asks the service to end the wait, and the task stays until `until`. */
  wait?: { waitId: string; expiresAt: number; delegation?: { until: number } };
  /** The answer the task continued with. A later recovery of that attempt hands the same answer to the worker. */ answered?: { waitId: string; messageId: string; text: string; /** The decision, when the wait was an approval. */ approved?: boolean } }
/** What a Local App call handed back to the worker: whether the app answered, and exactly what the worker was given. */
export interface AppCallOutcome { succeeded: boolean; result: Record<string, unknown> }
/** One Local App call of a task: written before the app is asked and completed with its outcome. Without an outcome the call began and how it ended is not known. */
export interface SavedAppCall { schemaVersion: 1; taskId: string; step: string; requestDigest: string; outcome?: AppCallOutcome }
/** `begun`: an earlier process began this call and left no outcome. `conflict`: the step is the same and the call is not. `full`: the task has made as many calls as it may. `refused`: new, and not admitted. */
export type AppCallEntry = { state: 'new' | 'begun' | 'conflict' | 'full' | 'refused' } | { state: 'done'; outcome: AppCallOutcome };
/** The most calls of Local Apps one task may make, and the most bytes of one record of such a call. */
const APP_CALLS_PER_TASK = 64, APP_CALL_BYTES = 80 * 1024;
export interface SavedDelivery { schemaVersion: 1; teamId: string; generation: number; taskId: string; deadlineAt: number; commandId: string; payload: Record<string, unknown>;
  /** The host made this failure up (no worker result), so its `finishedAt` is a host clock stamp that delivery may replace. */ synthesized?: true;
  /** The result waits for the Bot's maintainer: it is handed to the service as `holdReply` (same payload) and posted by their approval, not by `completeTask`. */ hold?: true }

export interface TeamModelSelection { route: TeamModelRoute; profiles: Record<string, unknown>[]; assertCurrent(): Promise<void> }

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
    const value = Buffer.alloc(info.size + 1), length = readSync(fd, value, 0, value.length, 0);
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
    const bytes = Buffer.alloc(info.size + 1);
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
const TEMPORARY = '.write-';
const listed = async (root: string): Promise<string[]> => (await readdir(root)).filter(name => !name.startsWith(TEMPORARY));
async function writeTemporary(root: string, value: string): Promise<string> {
  const path = join(root, `${TEMPORARY}${randomBytes(16).toString('hex')}`);
  const file = await open(path, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try { await file.writeFile(value); await file.sync(); } finally { await file.close(); }
  return path;
}
/** Create-once and atomic: the complete file is linked into place (EEXIST if it already exists), so an exit mid-write leaves at most a skipped temporary file. */
async function create(path: string, value: string): Promise<void> {
  const temporary = await writeTemporary(dirname(path), value);
  try { await link(temporary, path); } finally { await unlink(temporary).catch(() => undefined); }
  await syncDirectory(dirname(path));
}
function bindingText(value: SharedBotBinding, uid: string): string {
  const ids = [value.teamId, value.sharedBotId, value.deploymentId, value.instanceId, value.publicationId];
  if (!value || Object.keys(value).sort().join(',') !== 'accountEpoch,deploymentId,generation,grantIds,instanceId,ownerUid,publicationId,revision,schemaVersion,sessionEpoch,sharedBotId,snapshot,teamId'
    || value.schemaVersion !== 1 || value.ownerUid !== uid || !/^[A-Za-z0-9_-]{1,128}$/u.test(uid)
    || ids.some(id => typeof id !== 'string' || !/^[A-Za-z0-9_-]{8,64}$/u.test(id))
    || [value.generation, value.revision, value.accountEpoch, value.sessionEpoch].some(n => !Number.isSafeInteger(n) || n < 1)
    || !/^[a-f0-9]{64}$/u.test(value.snapshot) || !Array.isArray(value.grantIds) || value.grantIds.length > 50
    || value.grantIds.some(id => !/^[A-Za-z0-9_-]{8,64}$/u.test(id))) throw new Error('UNSAFE_TEAM_STORAGE');
  return JSON.stringify(value);
}

/** Execution 2 storage is independent of personal definitions and retired host data. */
export class TeamPrivateStore {
  readonly root: string;
  readonly hostId: string;
  private operation: Promise<unknown> = Promise.resolve();
  private serial<T>(run: () => Promise<T>): Promise<T> {
    const result = this.operation.then(run); this.operation = result.catch(() => undefined); return result;
  }
  private executionSerial<T>(uid:string,taskId:string,run:()=>Promise<T>):Promise<T>{
    return this.serial(async()=>withProviderAccountLock(await directory(join((await this.owner(uid)).root,'execution-locks')),digest(taskId),run));
  }
  constructor(userData: string) {
    this.root = privateDirectory(join(realpathSync.native(userData), 'shared-bot-host-v2'));
    const path = join(this.root, 'host-id');
    try { createPrivate(path, randomBytes(16).toString('base64url')); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    this.hostId = readPrivate(path, 22);
    if (!/^[A-Za-z0-9_-]{22}$/u.test(this.hostId)) throw new Error('UNSAFE_TEAM_STORAGE');
  }
  async owner(uid: string): Promise<{ root: string; runtime: string; snapshots: string; bindings: string; transport: string; publications: string; executions: string; outbox: string; appCalls: string; home: string; tmp: string }> {
    if (!/^[A-Za-z0-9_-]{1,128}$/u.test(uid)) throw new Error('BOT_ACCOUNT_UNSUPPORTED');
    await directory(this.root); const root = await directory(join(this.root, digest(uid)));
    return { root, runtime: await directory(join(root, 'runtime')), snapshots: await directory(join(root, 'snapshots')),
      bindings: await directory(join(root, 'bindings')), transport: await directory(join(await directory(join(root, 'runtime')), 'transport')),
      publications: await directory(join(root, 'publications')), executions: await directory(join(root, 'executions')), outbox: await directory(join(root, 'outbox')), appCalls: await directory(join(root, 'app-calls')), home: await directory(join(root, 'host-home')), tmp: await directory(join(root, 'host-tmp')) };
  }
  async publication(uid: string, publicationId: string): Promise<SavedPublication | undefined> {
    return this.readPublication(uid, (await this.owner(uid)).publications, publicationId);
  }
  private async readPublication(uid: string, root: string, publicationId: string): Promise<SavedPublication | undefined> {
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(publicationId)) throw new Error('INVALID_INPUT');
    try { const value = JSON.parse(await read(join(root,publicationId),128*1024)) as SavedPublication;
      if (value.schemaVersion !== 1 || !/^[a-f0-9]{64}$/u.test(value.requestDigest) || value.binding.publicationId !== publicationId || (value.action !== undefined && value.action !== 'completeTransfer')) throw new Error('UNSAFE_TEAM_STORAGE'); bindingText(value.binding,uid); return value;
    } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; }
  }
  async publications(uid: string): Promise<SavedPublication[]> {
    const root=(await this.owner(uid)).publications, values:SavedPublication[]=[];
    for (const name of await listed(root)) { const value=await this.readPublication(uid,root,name); if(value)values.push(value); }
    return values;
  }
  async savePublication(uid: string, value: SavedPublication): Promise<void> {
    bindingText(value.binding,uid); const text = JSON.stringify(value),path=join((await this.owner(uid)).publications,value.binding.publicationId);
    if (Buffer.byteLength(text)>128*1024) throw new Error('PUBLICATION_TOO_LARGE');
    try { await create(path,text); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    if (await read(path,128*1024)!==text) throw new Error('UNSAFE_TEAM_STORAGE');
  }
  saveBinding(uid: string, value: SharedBotBinding): Promise<void> { return this.serial(async () => {
    const text = bindingText(value, uid), root = (await this.owner(uid)).bindings, path = join(root, digest(JSON.stringify([value.teamId, value.sharedBotId])));
    try {
      const previous=JSON.parse(await read(path,64*1024)) as SharedBotBinding; bindingText(previous,uid);
      if (previous.revision>value.revision || previous.revision===value.revision && (previous.accountEpoch>value.accountEpoch
        || JSON.stringify({...previous,accountEpoch:value.accountEpoch})!==text)) throw new Error('STALE_PUBLICATION_BINDING');
    } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
    await rename(await writeTemporary(root, text), path); await syncDirectory(root);
  }); }
  bindings(uid: string): Promise<SharedBotBinding[]> { return this.serial(async () => {
    const root = (await this.owner(uid)).bindings, names = await listed(root);
    if (names.length > 1000) throw new Error('UNSAFE_TEAM_STORAGE');
    const values: SharedBotBinding[] = [];
    for (const name of names) {
      if (!/^[a-f0-9]{64}$/u.test(name)) throw new Error('UNSAFE_TEAM_STORAGE');
      const text = await read(join(root, name), 64 * 1024), value = JSON.parse(text) as SharedBotBinding;
      if (bindingText(value, uid) !== text || digest(JSON.stringify([value.teamId, value.sharedBotId])) !== name) throw new Error('UNSAFE_TEAM_STORAGE');
      values.push(value);
    }
    return values;
  }); }
  saveSnapshot(uid: string, value: TeamPersonaSnapshot): Promise<string> { return this.serial(async () => {
    this.validateSnapshot(value); const text = JSON.stringify(value);
    if (Buffer.byteLength(text) > 4 * 1024 * 1024) throw new Error('INVALID_TEAM_SNAPSHOT');
    const key = digest(text), path = join((await this.owner(uid)).snapshots, key);
    try { await create(path, text); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    if (await read(path, 4 * 1024 * 1024) !== text) throw new Error('INVALID_TEAM_SNAPSHOT'); return key;
  }); }
  snapshot(uid: string, key: string): Promise<TeamPersonaSnapshot> { return this.serial(async () => {
    if (!/^[a-f0-9]{64}$/u.test(key)) throw new Error('INVALID_TEAM_SNAPSHOT');
    const text = await read(join((await this.owner(uid)).snapshots, key), 4 * 1024 * 1024);
    if (digest(text) !== key) throw new Error('INVALID_TEAM_SNAPSHOT');
    const value = JSON.parse(text) as TeamPersonaSnapshot; this.validateSnapshot(value); return value;
  }); }
  async privateDataTask(uid: string, principal: { taskId: string; requesterUid: string; teamId: string; sharedBotId: string }): Promise<string> {
    if (![principal.taskId, principal.teamId, principal.sharedBotId].every(value => /^[A-Za-z0-9_-]{8,64}$/u.test(value))
      || !/^[A-Za-z0-9_-]{1,128}$/u.test(principal.requesterUid)) throw new Error('INVALID_PRIVATE_DATA_TASK');
    const run = await directory(join(await directory(join((await this.owner(uid)).root, 'data-private')), principal.taskId));
    const admission = join(run, 'admission.json');
    const identity = JSON.stringify([1, uid, principal.requesterUid, principal.teamId, principal.sharedBotId, principal.taskId]);
    try { await create(admission, identity); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    if (await read(admission, 2048) !== identity) throw new Error('INVALID_PRIVATE_DATA_TASK');
    return run;
  }

  async privateDataInput(uid: string, principal: Parameters<TeamPrivateStore['privateDataTask']>[1], value: unknown): Promise<TeamDataRef> {
    const run = await this.privateDataTask(uid, principal);
    return this.writeDataRef(await directory(join(run, 'transport')), value);
  }

  async releasePrivateDataInput(uid: string, principal: Parameters<TeamPrivateStore['privateDataTask']>[1], reference: TeamDataRef): Promise<void> {
    const run = await this.privateDataTask(uid, principal);
    const root = await directory(join(run, 'transport'));
    if (!this.transportPath(root, reference)) throw new Error('UNSAFE_TEAM_STORAGE');
    await unlink(reference.path).catch(error => { if (error.code !== 'ENOENT') throw error; });
  }

  /** Immutable, digest checked transport spool. Runtime journal remains the input authority. */
  async dataRef(uid: string, value: unknown): Promise<TeamDataRef> {
    return this.writeDataRef((await this.owner(uid)).transport, value);
  }

  private async writeDataRef(root: string, value: unknown): Promise<TeamDataRef> {
    const text = JSON.stringify(value), sha256 = digest(text), bytes = Buffer.byteLength(text);
    if (bytes > 64 * 1024 * 1024) throw new Error('TEAM_INPUT_CAPACITY');
    const path = join(root, `${sha256}-${randomBytes(16).toString('hex')}`);
    try { await create(path, text); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    if (await read(path, bytes) !== text) throw new Error('UNSAFE_TEAM_STORAGE'); return { path, sha256, bytes };
  }
  async releaseDataRef(uid: string, reference: TeamDataRef): Promise<void> {
    const root = (await this.owner(uid)).transport;
    if (!this.transportPath(root, reference)) throw new Error('UNSAFE_TEAM_STORAGE');
    await unlink(reference.path).catch(error => { if (error.code !== 'ENOENT') throw error; });
  }
  /** Atomic replace: the record is whole at every instant, and a later attempt never goes back to an earlier one. */
  saveExecution(uid: string, value: SavedExecution): Promise<void> { return this.executionSerial(uid,value.taskId,async () => {
    const paths = await this.owner(uid), root = paths.executions, path = join(root, value.taskId), text = JSON.stringify(value);
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(value.taskId) || value.schemaVersion !== 2 || !Number.isSafeInteger(value.epoch) || value.epoch < 1
      || Buffer.byteLength(text) > 64 * 1024 || !this.transportPath(paths.transport, value.batch)) throw new Error('UNSAFE_TEAM_STORAGE');
    try { const previous = JSON.parse(await read(path, 64 * 1024)) as SavedExecution; if (previous.schemaVersion === 2 && (previous.epoch > value.epoch
      || (previous.grant||value.grant) && JSON.stringify(previous.grant) !== JSON.stringify(value.grant))) throw new Error('STALE_EXECUTION'); }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; if(value.grant)throw new Error('STALE_EXECUTION'); }
    await rename(await writeTemporary(root, text), path); await syncDirectory(root);
  }); }
  /** Single-instance Desktop owns this store. Initial intent uses create-no-replace; all transitions compare the complete prior record. */
  compareExecution(uid: string, before: SavedExecution | undefined, next: SavedExecution): Promise<boolean> { return this.executionSerial(uid,next.taskId,async () => {
    const paths = await this.owner(uid), path = join(paths.executions,next.taskId), text = JSON.stringify(next);
    if (next.schemaVersion !== 2 || !/^[A-Za-z0-9_-]{8,64}$/u.test(next.taskId) || !Number.isSafeInteger(next.epoch) || next.epoch < 1
      || Buffer.byteLength(text)>64*1024 || !this.transportPath(paths.transport,next.batch)) throw new Error('UNSAFE_TEAM_STORAGE');
    if (next.grant && (!/^[a-f0-9]{64}$/u.test(next.grant.nonce) || !Number.isSafeInteger(next.grant.targetEpoch) || next.grant.targetEpoch < 1
      || next.grant.rpcAttempts!==undefined&&(!Number.isSafeInteger(next.grant.rpcAttempts)||next.grant.rpcAttempts<0)
      || next.grant.binding.ownerUid!==uid || next.grant.binding.teamId!==next.teamId || next.grant.payload.taskId!==next.taskId)) throw new Error('UNSAFE_TEAM_STORAGE');
    try { await lstat(join(paths.outbox,next.taskId)); return false; } catch(error) { if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error; }
    let current: string | undefined; try { current=await read(path,64*1024); } catch(error) { if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error; }
    if (before ? current!==JSON.stringify(before) : current!==undefined) return false;
    if (before) { await rename(await writeTemporary(paths.executions,text),path); await syncDirectory(paths.executions); }
    else { try { await create(path,text); } catch(error) { if((error as NodeJS.ErrnoException).code==='EEXIST')return false;throw error; } }
    return true;
  }); }
  discardExecution(uid: string, expected: SavedExecution, restore?: SavedExecution): Promise<boolean> { return this.executionSerial(uid,expected.taskId,async () => {
    const paths=await this.owner(uid),path=join(paths.executions,expected.taskId);
    let current:string;try{current=await read(path,64*1024);}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return false;throw error;}
    if(current!==JSON.stringify(expected))return false;
    try{await lstat(join(paths.outbox,expected.taskId));return false;}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error;}
    if(restore)await rename(await writeTemporary(paths.executions,JSON.stringify(restore)),path);else await unlink(path);
    await syncDirectory(paths.executions);
    if(!restore){await this.releaseDataRef(uid,expected.batch);await rm(join(paths.appCalls,expected.taskId),{recursive:true,force:true});}return true;
  }); }
  async executions(uid: string): Promise<SavedExecution[]> {
    const root = (await this.owner(uid)).executions, values: SavedExecution[] = [];
    for (const name of await listed(root)) {
      if (!/^[A-Za-z0-9_-]{8,64}$/u.test(name)) throw new Error('UNSAFE_TEAM_STORAGE');
      let text: string; try { text = await read(join(root,name),64*1024); } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue; throw error; }
      const value = JSON.parse(text) as SavedExecution;
      // The retired single-attempt record kept no input batch, so its task cannot be continued or closed from here: it is removed.
      if ((value as { schemaVersion?: unknown }).schemaVersion === 1) { await unlink(join(root, name)).catch(() => undefined); continue; }
      if (value.schemaVersion !== 2 || value.taskId !== name) throw new Error('UNSAFE_TEAM_STORAGE'); values.push(value);
    }
    return values;
  }
  private transportPath(root: string, reference: TeamDataRef): boolean {
    return !!reference && typeof reference.path === 'string' && typeof reference.sha256 === 'string' && /^[a-f0-9]{64}$/u.test(reference.sha256) && dirname(reference.path) === root
      && new RegExp(`^${reference.sha256}-[a-f0-9]{32}$`, 'u').test(reference.path.slice(root.length + 1));
  }
  /** Whether a saved execution still waits for recovery: its intent is saved and its delivery is not. Only two names are looked up, so a run that cleans up meanwhile reads as done. */
  async awaitingRecovery(uid: string, taskId: string): Promise<boolean> {
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(taskId)) throw new Error('UNSAFE_TEAM_STORAGE');
    const paths = await this.owner(uid), exists = (path: string) => lstat(path).then(() => true, (error: NodeJS.ErrnoException) => { if (error.code === 'ENOENT') return false; throw error; });
    return await exists(join(paths.executions, taskId)) && !await exists(join(paths.outbox, taskId));
  }
  async savedResult(uid: string, taskId: string): Promise<TeamRunResult | undefined> {
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(taskId)) throw new Error('UNSAFE_TEAM_STORAGE');
    const root = (await this.owner(uid)).runtime;
    try {
      const results=join(root,'results'); const info=await lstat(results);
      if (!info.isDirectory() || (info.mode&0o077)!==0 || process.getuid && info.uid!==process.getuid()) throw new Error('UNSAFE_TEAM_STORAGE');
      const result = JSON.parse(await read(join(results,`${taskId}.json`),512*1024));
      // A Bot that chose silence leaves a result too: no text and no document.
      if (!result || result.version !== 3 || result.type !== 'result' || !['completed','failed','declined'].includes(result.status)
        || result.status === 'declined' && (result.text !== '' || result.document !== undefined)
        || typeof result.text !== 'string' || Buffer.byteLength(result.text)>8192 || !Number.isSafeInteger(result.finishedAt) || !result.usage || !['missing','partial','complete'].includes(result.usage.status)) throw new Error('UNSAFE_TEAM_STORAGE');
      return result as TeamRunResult;
    } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; }
  }
  saveDelivery(uid: string, value: SavedDelivery, expected?: SavedExecution): Promise<boolean> { return this.executionSerial(uid,value.taskId,async () => {
    const path = join((await this.owner(uid)).outbox, value.taskId), text = JSON.stringify(value);
    if(expected){let current:SavedExecution;try{current=JSON.parse(await read(join((await this.owner(uid)).executions,value.taskId),64*1024));}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return false;throw error;}
      if(current.epoch!==expected.epoch||current.payload.permitId!==expected.payload.permitId||JSON.stringify(current.grant)!==JSON.stringify(expected.grant))return false;}
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(value.taskId) || text.length > 512 * 1024) throw new Error('UNSAFE_TEAM_STORAGE');
    try { await create(path, text); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
    if (await read(path, 512 * 1024) !== text) throw new Error('UNSAFE_TEAM_STORAGE');
    return true;
  }); }
  deliveries(uid: string): Promise<SavedDelivery[]> { return this.serial(async () => {
    const root = (await this.owner(uid)).outbox, values: SavedDelivery[] = [];
    for (const name of await listed(root)) {
      if (!/^[A-Za-z0-9_-]{8,64}$/u.test(name)) throw new Error('UNSAFE_TEAM_STORAGE');
      const value = JSON.parse(await read(join(root, name), 512 * 1024)) as SavedDelivery;
      if (value.schemaVersion !== 1 || value.taskId !== name || !/^[A-Za-z0-9_-]{8,64}$/u.test(value.teamId) || !Number.isSafeInteger(value.deadlineAt)) throw new Error('UNSAFE_TEAM_STORAGE'); values.push(value);
    }
    return values;
  }); }
  /** The task is over for this host: its delivery, its execution record and the input batch the record kept are removed. */
  delivered(uid: string, taskId: string, expected?: SavedExecution): Promise<boolean> { return this.executionSerial(uid,taskId,async () => {
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(taskId)) throw new Error('UNSAFE_TEAM_STORAGE');
    const paths = await this.owner(uid), root = paths.outbox;
    if(expected){let current:SavedExecution;try{current=JSON.parse(await read(join(paths.executions,taskId),64*1024));}catch(error){if((error as NodeJS.ErrnoException).code==='ENOENT')return false;throw error;}
      if(current.epoch!==expected.epoch||current.payload.permitId!==expected.payload.permitId||JSON.stringify(current.grant)!==JSON.stringify(expected.grant))return false;}
    await unlink(join(root, taskId)).catch(error => { if (error.code !== 'ENOENT') throw error; }); await syncDirectory(root);
    let batch: TeamDataRef | undefined;
    try { batch = (JSON.parse(await read(join(paths.executions, taskId), 64 * 1024)) as SavedExecution).batch; } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
    await unlink(join(paths.executions, taskId)).catch(error => { if (error.code !== 'ENOENT') throw error; }); await syncDirectory(paths.executions);
    if (batch && this.transportPath(paths.transport, batch)) await unlink(batch.path).catch(error => { if (error.code !== 'ENOENT') throw error; });
    await rm(join(paths.appCalls, taskId), { recursive: true, force: true });
    return true;
  }); }
  /**
   * Begins one Local App call of a task, or says what an earlier process left of it. The record is made durable before the app is asked, so a call is never made twice:
   * a process that continues the task and sends the same step again is given the recorded outcome, or `begun` when the call started and its end was never recorded.
   * `admitted: false` makes no record: a call that is refused leaves nothing behind.
   */
  beginAppCall(uid: string, taskId: string, step: string, requestDigest: string, admitted: boolean): Promise<AppCallEntry> { return this.executionSerial(uid, taskId, async () => {
    const root = await this.appCallRoot(uid, taskId, step, requestDigest), path = join(root, step), saved = await this.readAppCall(path, taskId, step);
    if (saved) return saved.requestDigest !== requestDigest ? { state: 'conflict' } : saved.outcome ? { state: 'done', outcome: saved.outcome } : { state: 'begun' };
    if (!admitted) return { state: 'refused' };
    if ((await listed(root)).length >= APP_CALLS_PER_TASK) return { state: 'full' };
    await create(path, JSON.stringify({ schemaVersion: 1, taskId, step, requestDigest } satisfies SavedAppCall));
    return { state: 'new' };
  }); }
  /** Records how a begun call ended: exactly what the worker is given, now and for every later send of the step. */
  finishAppCall(uid: string, taskId: string, step: string, requestDigest: string, outcome: AppCallOutcome): Promise<void> { return this.executionSerial(uid, taskId, async () => {
    const root = await this.appCallRoot(uid, taskId, step, requestDigest), path = join(root, step), saved = await this.readAppCall(path, taskId, step);
    if (!saved || saved.requestDigest !== requestDigest || saved.outcome) throw new Error('UNSAFE_TEAM_STORAGE');
    const text = JSON.stringify({ ...saved, outcome });
    if (Buffer.byteLength(text) > APP_CALL_BYTES) throw new Error('UNSAFE_TEAM_STORAGE');
    await rename(await writeTemporary(root, text), path); await syncDirectory(root);
  }); }
  /** Takes a begun call back: the app was never asked (the call was refused before it was made), so the step may be tried again. */
  dropAppCall(uid: string, taskId: string, step: string): Promise<void> { return this.executionSerial(uid, taskId, async () => {
    const root = await this.appCallRoot(uid, taskId, step);
    await unlink(join(root, step)).catch(error => { if (error.code !== 'ENOENT') throw error; }); await syncDirectory(root);
  }); }
  /** The directory of a task's Local App calls, for a task and digests that have the shape the store writes. */
  private async appCallRoot(uid: string, taskId: string, ...digests: string[]): Promise<string> {
    if (!/^[A-Za-z0-9_-]{8,64}$/u.test(taskId) || !digests.every(value => /^[a-f0-9]{64}$/u.test(value))) throw new Error('UNSAFE_TEAM_STORAGE');
    return directory(join((await this.owner(uid)).appCalls, taskId));
  }
  private async readAppCall(path: string, taskId: string, step: string): Promise<SavedAppCall | undefined> {
    try {
      const value = JSON.parse(await read(path, APP_CALL_BYTES)) as SavedAppCall, outcome = value.outcome;
      if (value.schemaVersion !== 1 || value.taskId !== taskId || value.step !== step || !/^[a-f0-9]{64}$/u.test(value.requestDigest)
        || outcome !== undefined && (typeof outcome.succeeded !== 'boolean' || !outcome.result || typeof outcome.result !== 'object' || Array.isArray(outcome.result))) throw new Error('UNSAFE_TEAM_STORAGE');
      return value;
    } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; }
  }
  private validateSnapshot(value: TeamPersonaSnapshot): void {
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('INVALID_TEAM_SNAPSHOT');
    const { instructions, documents, provider_account_profiles: profiles, ...route } = value; validateTeamModelRoute(route);
    if (!Array.isArray(profiles) || !profiles.length || profiles.length > 64 || profiles.some(p => !p || typeof p !== 'object' || Array.isArray(p))
      || !profiles.some(p => JSON.stringify(p) === JSON.stringify(route.model_profile)) || typeof instructions !== 'string' || !instructions.trim()
      || instructions.includes('\0') || Buffer.byteLength(instructions) > 64 * 1024 || !Array.isArray(documents) || documents.length < 1 || documents.length > 50
      || documents.some(d => !/^[A-Za-z0-9_-]{8,64}$/u.test(d.resourceId) || !Number.isSafeInteger(d.revision) || d.revision < 1
        || !['persona','file','skill'].includes(d.kind) || typeof d.text !== 'string' || d.text.includes('\0') || digest(d.text) !== d.digest
        || typeof d.name !== 'string' || !d.name || d.name.length > 255)) throw new Error('INVALID_TEAM_SNAPSHOT');
  }
}
