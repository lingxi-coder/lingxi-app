/**
 * The Bots of this computer, on disk. Free of Electron.
 *
 *     <userData>/bots/<botId>/bot.json     the record (the last file written when a Bot is made)
 *                            home/         the Bot's configuration home: LINGXI.md, settings.json, skills/, its sessions and memory
 *                            workspace/    the folder the Bot's sessions run in
 *                            tasks/        the tasks other Bots handed this one, a file each
 *     <userData>/bots-trash/<botId>-<ms>/  a deleted Bot, kept until its owner removes it
 *
 * The root is deliberately outside the default configuration home (`~/.lingxi`):
 * nothing a normal session sees lists, reads or writes a Bot, and nothing a Bot
 * sees is the owner's own configuration.
 */

import { randomBytes } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync, statSync } from 'node:fs';
import { basename, dirname, isAbsolute, join, parse, resolve } from 'node:path';
import type { PermissionModeId } from '@lingxi/bridge-client';

import { BOT_CONFLICT_MESSAGE } from '../../shared/bots.js';
import { isPermissionModeId } from '../host-utils.js';
import { isSessionId } from '../sessionIdentity.js';
import { isInside, writeFileAtomic } from './files.js';
import { projectDirName } from './projectDirName.js';

export const BOT_RECORD_VERSION = 1;
export const MAX_BOT_WORKSPACES = 16;
export const MAX_ACCEPT_FROM = 64;
export const MAX_PERSONA_BYTES = 64 * 1024;
const MAX_RECORD_BYTES = 64 * 1024;
const MAX_NAME_CHARS = 80;
const MAX_DESCRIPTION_CHARS = 500;
const MAX_MODEL_CHARS = 256;
const MAX_PATH_CHARS = 4096;
const BOT_ID = /^[A-Za-z0-9_-]{22}$/;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/;

export type BotWorkspaceMode = 'read' | 'write';

/** Host-owned Team bindings. These never grant access to the ordinary Bot home. */
export interface BotTeamBinding {
  ownerUid: string; teamId: string; generation: number; botKey: string; revision: number;
  snapshot: string; use: 'all' | 'listed'; uids: string[];
}

export interface BotWorkspace {
  /** The real path of an existing directory. */
  path: string;
  mode: BotWorkspaceMode;
}

/** `bot.json`, version 1. */
export interface BotRecord {
  v: 1;
  bot_id: string;
  name: string;
  description: string;
  /** Epoch milliseconds. */
  created_at: number;
  updated_at: number;
  revision: number;
  model?: string;
  /** What a session starts in; one of the modes the Desktop offers. Absent: the engine's default. */
  permission_mode?: PermissionModeId;
  /** The Bots that may delegate tasks to this one, by id. Absent: none may. */
  accept_from?: string[];
  /** Whether the owner's approved devices may reach this Bot's chats through Remote Control. Absent: they may not. */
  remote?: boolean;
  teams?: BotTeamBinding[];
  workspaces: BotWorkspace[];
  /** Only the owner exists in this version. */
  audiences: Array<{ id: 'owner'; kind: 'owner' }>;
}

export interface BotCreateInput {
  name: string;
  description?: string;
  persona?: string;
  model?: string;
  permissionMode?: PermissionModeId;
  acceptFrom?: ReadonlyArray<string>;
  workspaces?: ReadonlyArray<{ path: string; mode: BotWorkspaceMode }>;
}

export interface BotPatch {
  name?: string;
  description?: string;
  /** `null` removes the Bot's own model. */
  model?: string | null;
  /** `null` removes the Bot's own permission mode. */
  permissionMode?: PermissionModeId | null;
  /** The Bots that may delegate tasks to this one; an empty list removes them all. */
  acceptFrom?: ReadonlyArray<string>;
  /** Whether Remote Control reaches this Bot. */
  remote?: boolean;
  workspaces?: ReadonlyArray<{ path: string; mode: BotWorkspaceMode }>;
}

export interface BotStoreOptions {
  /** The app's data folder. */
  userData: string;
  /** The user's home directory. */
  home: string;
  /** The configuration home normal sessions use. */
  defaultConfigHome: string;
  now?: () => number;
  newId?: () => string;
}

/** The places the isolation of Bots is about, with every spelling they have. */
export interface BotLocations {
  userData: string;
  /** `<userData>/bots`, a real path. */
  root: string;
  trashRoot: string;
  home: string;
  /** The default configuration home: as given and, if it exists, as it really is. */
  defaultConfigHomes: string[];
  /** The global configuration file of the default home (`~/.lingxi.json`). */
  defaultGlobalConfigs: string[];
}

export class BotNotFoundError extends Error {
  constructor() {
    super('Bot not found');
    this.name = 'BotNotFoundError';
  }
}

export class BotConflictError extends Error {
  constructor() {
    super(BOT_CONFLICT_MESSAGE);
    this.name = 'BotConflictError';
  }
}

function plainObject(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error(`invalid ${what}`);
  return value as Record<string, unknown>;
}

function exactKeys(value: Record<string, unknown>, required: readonly string[], optional: readonly string[], what: string): void {
  if (required.some((key) => !(key in value)) || Object.keys(value).some((key) => !required.includes(key) && !optional.includes(key))) {
    throw new Error(`${what} has unsupported fields`);
  }
}

function singleLine(value: unknown, what: string, max: number): string {
  if (typeof value !== 'string') throw new Error(`invalid ${what}`);
  const trimmed = value.trim();
  if (trimmed.length === 0 || trimmed.length > max || CONTROL.test(trimmed)) throw new Error(`invalid ${what}`);
  return trimmed;
}

function botName(value: unknown): string {
  return singleLine(value, 'name', MAX_NAME_CHARS);
}

function botDescription(value: unknown): string {
  if (typeof value !== 'string' || value.length > MAX_DESCRIPTION_CHARS || value.includes('\0')) throw new Error('invalid description');
  return value.trim();
}

function botModel(value: unknown): string {
  return singleLine(value, 'model', MAX_MODEL_CHARS);
}

function botPermissionMode(value: unknown): PermissionModeId {
  if (!isPermissionModeId(value)) throw new Error('invalid permission mode');
  return value;
}

function botRemote(value: unknown): boolean {
  if (typeof value !== 'boolean') throw new Error('invalid remote');
  return value;
}

/** The text of a Bot's `LINGXI.md`. */
export function personaText(value: unknown): string {
  if (typeof value !== 'string' || value.includes('\0') || Buffer.byteLength(value, 'utf8') > MAX_PERSONA_BYTES) throw new Error('invalid persona');
  return value;
}

function timestamp(value: unknown, what: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) throw new Error(`invalid ${what}`);
  return value as number;
}

export function isBotId(value: unknown): value is string {
  return typeof value === 'string' && BOT_ID.test(value);
}

/** The Bots allowed to delegate to a Bot: valid ids, each once, not the Bot itself. */
function acceptFromList(value: unknown, selfId?: string): string[] {
  if (!Array.isArray(value) || value.length > MAX_ACCEPT_FROM || !value.every(isBotId) || new Set(value).size !== value.length
    || (selfId !== undefined && value.includes(selfId))) throw new Error('invalid accept_from');
  return [...value];
}

function sameBotIds(left: readonly string[] | undefined, right: readonly string[] | undefined): boolean {
  return JSON.stringify([...(left ?? [])].sort()) === JSON.stringify([...(right ?? [])].sort());
}

function parseWorkspaceEntry(value: unknown): { path: string; mode: BotWorkspaceMode } {
  const entry = plainObject(value, 'workspace');
  exactKeys(entry, ['path', 'mode'], [], 'workspace');
  if (typeof entry['path'] !== 'string' || entry['path'].length === 0 || entry['path'].length > MAX_PATH_CHARS || entry['path'].includes('\0')
    || !isAbsolute(entry['path'])) throw new Error('invalid workspace path');
  if (entry['mode'] !== 'read' && entry['mode'] !== 'write') throw new Error('invalid workspace mode');
  return { path: entry['path'], mode: entry['mode'] };
}

/** Read `bot.json` strictly: the one version, the exact fields, nothing else. */
export function parseRecord(text: string, expectedId: string): BotRecord {
  let parsed: unknown;
  try { parsed = JSON.parse(text); } catch { throw new Error('bot.json is not valid JSON'); }
  const input = plainObject(parsed, 'bot.json');
  if (input['v'] !== BOT_RECORD_VERSION) throw new Error('bot.json has an unsupported version');
  exactKeys(input, ['v', 'bot_id', 'name', 'description', 'created_at', 'updated_at', 'revision', 'workspaces', 'audiences'], ['model', 'permission_mode', 'accept_from', 'remote', 'teams'], 'bot.json');
  if (input['bot_id'] !== expectedId || !isBotId(input['bot_id'])) throw new Error('bot.json belongs to a different Bot');
  const name = botName(input['name']);
  if (name !== input['name']) throw new Error('invalid name');
  const revision = input['revision'];
  if (!Number.isSafeInteger(revision) || (revision as number) < 1) throw new Error('invalid revision');
  if (!Array.isArray(input['workspaces']) || input['workspaces'].length > MAX_BOT_WORKSPACES) throw new Error('invalid workspaces');
  const workspaces = input['workspaces'].map(parseWorkspaceEntry);
  if (new Set(workspaces.map((workspace) => workspace.path)).size !== workspaces.length) throw new Error('invalid workspaces');
  const audiences = input['audiences'];
  if (!Array.isArray(audiences) || audiences.length !== 1) throw new Error('invalid audiences');
  const owner = plainObject(audiences[0], 'audience');
  exactKeys(owner, ['id', 'kind'], [], 'audience');
  if (owner['id'] !== 'owner' || owner['kind'] !== 'owner') throw new Error('invalid audiences');
  const description = botDescription(input['description']);
  if (description !== input['description']) throw new Error('invalid description');
  const acceptFrom = input['accept_from'] === undefined ? [] : acceptFromList(input['accept_from'], expectedId);
  const remote = input['remote'] === undefined ? false : botRemote(input['remote']);
  return {
    v: BOT_RECORD_VERSION,
    bot_id: expectedId,
    name,
    description,
    created_at: timestamp(input['created_at'], 'created_at'),
    updated_at: timestamp(input['updated_at'], 'updated_at'),
    revision: revision as number,
    ...(input['model'] !== undefined ? { model: botModel(input['model']) } : {}),
    ...(input['permission_mode'] !== undefined ? { permission_mode: botPermissionMode(input['permission_mode']) } : {}),
    ...(acceptFrom.length > 0 ? { accept_from: acceptFrom } : {}),
    ...(remote ? { remote } : {}),
    ...(input['teams'] !== undefined ? { teams: parseTeamBindings(input['teams']) } : {}),
    workspaces,
    audiences: [{ id: 'owner', kind: 'owner' }],
  };
}

function parseTeamBindings(value: unknown): BotTeamBinding[] {
  if (!Array.isArray(value) || value.length > 100) throw new Error('invalid Team bindings');
  const entries = value.map(raw => {
    const entry = plainObject(raw, 'Team binding');
    exactKeys(entry, ['ownerUid', 'teamId', 'generation', 'botKey', 'revision', 'snapshot', 'use', 'uids'], [], 'Team binding');
    if (typeof entry.ownerUid !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/u.test(entry.ownerUid)
      || typeof entry.teamId !== 'string' || !/^[A-Za-z0-9_-]{8,64}$/u.test(entry.teamId)
      || typeof entry.botKey !== 'string' || !/^bot:[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_-]{22}:[A-Za-z0-9_-]{22}$/u.test(entry.botKey)
      || !Number.isSafeInteger(entry.generation) || (entry.generation as number) < 1
      || !Number.isSafeInteger(entry.revision) || (entry.revision as number) < 1
      || typeof entry.snapshot !== 'string' || !/^[a-f0-9]{64}$/u.test(entry.snapshot)
      || !['all', 'listed'].includes(entry.use as string) || !Array.isArray(entry.uids) || entry.uids.length > 200
      || !entry.uids.every(uid => typeof uid === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(uid))
      || new Set(entry.uids).size !== entry.uids.length) throw new Error('invalid Team binding');
    return entry as unknown as BotTeamBinding;
  });
  if (new Set(entries.map(entry => JSON.stringify([entry.ownerUid, entry.teamId]))).size !== entries.length) throw new Error('duplicate Team binding');
  return entries;
}

function serialize(record: BotRecord): string {
  return `${JSON.stringify(record, null, 2)}\n`;
}

function defaultPersona(name: string, description: string): string {
  return description.length > 0 ? `# ${name}\n\n${description}\n` : `# ${name}\n`;
}

function realDirectory(path: string): string {
  const real = realpathSync.native(path);
  if (!statSync(real).isDirectory()) throw new Error('not a directory');
  return real;
}

export class BotStore {
  readonly locations: BotLocations;
  private readonly now: () => number;
  private readonly newId: () => string;

  constructor(options: BotStoreOptions) {
    mkdirSync(options.userData, { recursive: true });
    const userData = realDirectory(options.userData);
    const root = join(userData, 'bots');
    mkdirSync(root, { recursive: true, mode: 0o700 });
    const home = realDirectory(options.home);
    const defaultConfigHomes = [resolve(options.defaultConfigHome)];
    try {
      const real = realpathSync.native(defaultConfigHomes[0]!);
      if (!defaultConfigHomes.includes(real)) defaultConfigHomes.push(real);
    } catch { /* A configuration home that does not exist yet has one spelling. */ }
    this.locations = {
      userData,
      root: realDirectory(root),
      trashRoot: join(userData, 'bots-trash'),
      home,
      defaultConfigHomes,
      defaultGlobalConfigs: [join(home, '.lingxi.json')],
    };
    this.now = options.now ?? Date.now;
    this.newId = options.newId ?? (() => randomBytes(16).toString('base64url'));
  }

  get root(): string { return this.locations.root; }

  /** Trusted main only: renderer Bot patches cannot manufacture Team bindings. */
  setTeamBindings(botId: string, bindings: BotTeamBinding[]): void {
    const record = this.get(botId);
    const teams = parseTeamBindings(bindings);
    const text = serialize({ ...record, teams, revision: record.revision + 1, updated_at: this.now() });
    if (Buffer.byteLength(text) > MAX_RECORD_BYTES) throw new Error('TEAM_BINDING_LIMIT');
    writeFileAtomic(this.recordPath(botId), text);
  }

  botDir(botId: string): string { return join(this.root, this.checkedId(botId)); }
  recordPath(botId: string): string { return join(this.botDir(botId), 'bot.json'); }
  homeDir(botId: string): string { return join(this.botDir(botId), 'home'); }
  workspaceDir(botId: string): string { return join(this.botDir(botId), 'workspace'); }
  /** `<bot>/tasks`: the tasks other Bots handed this one, a file each. They go to the trash with the Bot. */
  tasksDir(botId: string): string { return join(this.botDir(botId), 'tasks'); }

  /** `<home>/projects/<project dir>`: where the engine keeps this Bot's transcripts, and its memory. */
  transcriptsDir(botId: string): string {
    return join(this.homeDir(botId), 'projects', projectDirName(this.workspaceDir(botId)));
  }

  /** `<transcripts>/memdir`: where the engine keeps what this Bot remembers. */
  memoryDir(botId: string): string {
    return join(this.transcriptsDir(botId), 'memdir');
  }

  /**
   * The Bot a working directory would belong to if that Bot exists: the path
   * alone decides, so a Bot that was deleted still has its workspace named.
   */
  workspaceBotId(path: string): string | undefined {
    if (typeof path !== 'string' || !isAbsolute(path)) return undefined;
    const normalized = resolve(path);
    const botDir = dirname(normalized);
    if (basename(normalized) !== 'workspace' || dirname(botDir) !== this.root) return undefined;
    const botId = basename(botDir);
    return isBotId(botId) ? botId : undefined;
  }

  /** The Bot a working directory belongs to, if it is the workspace of an existing Bot. */
  botIdForWorkspace(path: string): string | undefined {
    const botId = this.workspaceBotId(path);
    return botId !== undefined && existsSync(join(this.botDir(botId), 'bot.json')) ? botId : undefined;
  }

  /**
   * The Bot whose home holds the transcript of a session, if any. The id must
   * be a session's: it becomes part of a file name.
   */
  botIdOfTranscript(sessionId: string): string | undefined {
    if (!isSessionId(sessionId)) return undefined;
    for (const botId of readdirSync(this.root)) {
      if (isBotId(botId) && existsSync(join(this.transcriptsDir(botId), `${sessionId}.jsonl`))) return botId;
    }
    return undefined;
  }

  /** Every other entry of the Bots folder, whatever it is: they are all out of bounds for a Bot. */
  otherEntries(botId: string): string[] {
    return readdirSync(this.root).filter((name) => name !== botId).sort().map((name) => join(this.root, name));
  }

  get(botId: string): BotRecord {
    const path = this.recordPath(botId);
    let text: string;
    try {
      if (statSync(path).size > MAX_RECORD_BYTES) throw new Error('bot.json is too large');
      text = readFileSync(path, 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') throw new BotNotFoundError();
      throw error;
    }
    return parseRecord(text, botId);
  }

  /** Every Bot folder with the Bot read from it, or why it cannot be read. A folder without `bot.json` is a Bot that was never finished and is left out. */
  entries(): Array<{ botId: string; record?: BotRecord; error?: Error }> {
    const entries: Array<{ botId: string; record?: BotRecord; error?: Error }> = [];
    for (const botId of readdirSync(this.root).sort()) {
      if (!isBotId(botId)) continue;
      try {
        entries.push({ botId, record: this.get(botId) });
      } catch (error) {
        if (error instanceof BotNotFoundError) continue;
        entries.push({ botId, error: error instanceof Error ? error : new Error(String(error)) });
      }
    }
    return entries;
  }

  /** Every Bot, oldest first. A Bot whose record is damaged is named in the error. */
  list(): BotRecord[] {
    const records: BotRecord[] = [];
    for (const entry of this.entries()) {
      if (entry.record === undefined) throw new Error(`Cannot load Bot ${entry.botId}: ${entry.error?.message ?? 'unreadable'}`);
      records.push(entry.record);
    }
    return records.sort((left, right) => left.created_at - right.created_at || left.bot_id.localeCompare(right.bot_id));
  }

  /**
   * Make a Bot. `prepare` runs once the folders exist and before the record is
   * written (the Bot's settings are made there); if it throws, nothing is left.
   */
  create(input: BotCreateInput, prepare?: (record: BotRecord) => void): BotRecord {
    const name = botName(input.name);
    const description = input.description === undefined ? '' : botDescription(input.description);
    const model = input.model === undefined ? undefined : botModel(input.model);
    const permissionMode = input.permissionMode === undefined ? undefined : botPermissionMode(input.permissionMode);
    const acceptFrom = this.acceptFrom(input.acceptFrom ?? [], undefined, []);
    const workspaces = this.workspaces(input.workspaces ?? []);
    const persona = input.persona === undefined ? defaultPersona(name, description) : personaText(input.persona);
    let botId = '';
    for (let attempt = 0; attempt < 5 && botId === ''; attempt += 1) {
      const candidate = this.newId();
      if (!isBotId(candidate)) throw new Error('invalid Bot id');
      try {
        mkdirSync(join(this.root, candidate), { mode: 0o700 });
        botId = candidate;
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
      }
    }
    if (botId === '') throw new Error('could not allocate a Bot id');
    try {
      mkdirSync(join(this.homeDir(botId), 'skills'), { recursive: true, mode: 0o700 });
      mkdirSync(this.workspaceDir(botId), { mode: 0o700 });
      writeFileAtomic(join(this.homeDir(botId), 'LINGXI.md'), persona);
      const now = this.now();
      const record: BotRecord = {
        v: BOT_RECORD_VERSION,
        bot_id: botId,
        name,
        description,
        created_at: now,
        updated_at: now,
        revision: 1,
        ...(model !== undefined ? { model } : {}),
        ...(permissionMode !== undefined ? { permission_mode: permissionMode } : {}),
        ...(acceptFrom.length > 0 ? { accept_from: acceptFrom } : {}),
        workspaces,
        audiences: [{ id: 'owner', kind: 'owner' }],
      };
      prepare?.(record);
      writeFileAtomic(this.recordPath(botId), serialize(record));
      return record;
    } catch (error) {
      rmSync(this.botDir(botId), { recursive: true, force: true });
      throw error;
    }
  }

  /** Change a Bot that is still at `expectedRevision`; a Bot changed meanwhile is a conflict. */
  update(botId: string, expectedRevision: number, patch: BotPatch, prepare?: (record: BotRecord) => void): BotRecord {
    if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 1) throw new Error('invalid revision');
    const current = this.get(botId);
    if (current.revision !== expectedRevision) throw new BotConflictError();
    const next: BotRecord = {
      ...current,
      ...(patch.name !== undefined ? { name: botName(patch.name) } : {}),
      ...(patch.description !== undefined ? { description: botDescription(patch.description) } : {}),
      ...(patch.workspaces !== undefined ? { workspaces: this.workspaces(patch.workspaces) } : {}),
    };
    if (patch.model === null) delete next.model;
    else if (patch.model !== undefined) next.model = botModel(patch.model);
    if (patch.permissionMode === null) delete next.permission_mode;
    else if (patch.permissionMode !== undefined) next.permission_mode = botPermissionMode(patch.permissionMode);
    if (patch.acceptFrom !== undefined) {
      const acceptFrom = this.acceptFrom(patch.acceptFrom, botId, current.accept_from ?? []);
      if (acceptFrom.length > 0) next.accept_from = acceptFrom;
      else delete next.accept_from;
    }
    if (patch.remote !== undefined) {
      if (botRemote(patch.remote)) next.remote = true;
      else delete next.remote;
    }
    const unchanged = (['name', 'description', 'model', 'permission_mode', 'remote'] as const).every((key) => next[key] === current[key])
      && sameBotIds(next.accept_from, current.accept_from)
      && JSON.stringify(next.workspaces) === JSON.stringify(current.workspaces);
    if (unchanged) return current;
    next.revision = current.revision + 1;
    next.updated_at = Math.max(this.now(), current.updated_at);
    prepare?.(next);
    writeFileAtomic(this.recordPath(botId), serialize(next));
    return next;
  }

  /** Move a Bot's folder to the trash folder. Nothing deletes it from there. */
  trash(botId: string): string {
    const source = this.botDir(botId);
    if (!existsSync(source)) throw new BotNotFoundError();
    mkdirSync(this.locations.trashRoot, { recursive: true, mode: 0o700 });
    let destination = join(this.locations.trashRoot, `${botId}-${this.now()}`);
    for (let suffix = 1; existsSync(destination); suffix += 1) destination = join(this.locations.trashRoot, `${botId}-${this.now()}-${suffix}`);
    renameSync(source, destination);
    return destination;
  }

  /**
   * A folder the owner wants a Bot to work in: absolute, an existing
   * directory, kept as its real path. Not the filesystem root, the home folder
   * or a folder that contains it, anything of the app's own data (the Bots
   * included), or the configuration home of normal sessions.
   */
  workspace(path: unknown, mode: unknown): BotWorkspace {
    const entry = parseWorkspaceEntry({ path, mode });
    let real: string;
    try { real = realDirectory(entry.path); } catch { throw new Error('The folder does not exist or is not a directory.'); }
    const { userData, root, home, defaultConfigHomes } = this.locations;
    if (real === parse(real).root) throw new Error('A Bot cannot work in the filesystem root.');
    if (isInside(home, real)) throw new Error('A Bot cannot work in your home folder or in a folder that contains it.');
    if (isInside(real, userData) || isInside(root, real)) throw new Error("A Bot cannot work in the app's own data folder.");
    if (defaultConfigHomes.some((configHome) => isInside(real, configHome) || isInside(configHome, real))) {
      throw new Error('A Bot cannot work in the LingXi configuration folder.');
    }
    return { path: real, mode: entry.mode };
  }

  /** The list of Bots that may delegate here, sorted: valid, once each, existing (or listed already), and not the Bot itself. */
  private acceptFrom(ids: ReadonlyArray<string>, selfId: string | undefined, listed: readonly string[]): string[] {
    const list = acceptFromList(ids, selfId);
    if (list.some((id) => !listed.includes(id) && !existsSync(this.recordPath(id)))) throw new Error('accept_from names a Bot that does not exist');
    return list.sort();
  }

  private workspaces(entries: ReadonlyArray<{ path: string; mode: BotWorkspaceMode }>): BotWorkspace[] {
    if (!Array.isArray(entries) || entries.length > MAX_BOT_WORKSPACES) throw new Error('invalid workspaces');
    const bound = entries.map((entry) => {
      const parsed = plainObject(entry, 'workspace');
      exactKeys(parsed, ['path', 'mode'], [], 'workspace');
      return this.workspace(parsed['path'], parsed['mode']);
    });
    if (new Set(bound.map((workspace) => workspace.path)).size !== bound.length) throw new Error('The same folder is bound twice.');
    return bound;
  }

  private checkedId(botId: string): string {
    if (!isBotId(botId)) throw new Error('invalid Bot id');
    return botId;
  }
}

/** A folder bound earlier that still is what it was: a real directory at the same real path, still allowed. */
export function stillBound(store: BotStore, workspace: BotWorkspace): boolean {
  try {
    if (lstatSync(workspace.path).isSymbolicLink()) return false;
    return store.workspace(workspace.path, workspace.mode).path === workspace.path;
  } catch {
    return false;
  }
}
