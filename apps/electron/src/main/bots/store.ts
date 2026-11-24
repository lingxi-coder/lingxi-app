/**
 * The Bots of this computer, on disk. Free of Electron.
 *
 *     <userData>/bots/<botId>/bot.json     the record (the last file written when a Bot is made)
 *                            home/         the Bot's configuration home: LINGXI.md, settings.json, skills/, its sessions and memory
 *                            workspace/    the Bot's own folder, the default one its sessions run in; the owner may add more
 *                            tasks/        the tasks other Bots handed this one, a file each
 *     <userData>/bots-trash/<botId>-<ms>/  a deleted Bot, kept until its owner removes it
 *
 * The root is deliberately outside the default configuration home (`~/.lingxi`)
 * and the brand workspace root (`~/.lingxi-workspaces`, where no-project chats,
 * Local App sources and cron runs live): nothing a normal session sees lists,
 * reads or writes a Bot, and nothing a Bot sees is the owner's own configuration
 * or working files.
 */

import { createHash, randomBytes } from 'node:crypto';
import type { BotMarketOrigin } from '../../shared/botMarket.js';
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync, statSync } from 'node:fs';
import { basename, dirname, isAbsolute, join, parse, resolve } from 'node:path';
import type { PermissionModeId } from '@lingxi/bridge-client';

import { BOT_CONFLICT_MESSAGE } from '../../shared/bots.js';
import { isPermissionModeId } from '../host-utils.js';
import { isSessionId } from '../sessionIdentity.js';
import { isInside, writeFileAtomic } from './files.js';
import { projectDirName } from './projectDirName.js';

export const BOT_RECORD_VERSION = 2;
/** The id of the Bot's own folder, `<bot>/workspace`: always there, never removed. */
export const DEFAULT_WORKSPACE_ID = 'default';
/** How many folders the owner may add to the Bot's own one. */
export const MAX_BOT_WORKSPACES = 16;
export const MAX_ACCEPT_FROM = 64;
export const MAX_PERSONA_BYTES = 64 * 1024;
export const MAX_ALLOWED_DOMAINS = 64;
export const MAX_SCOPE_ENTRIES = 64;
export const MAX_REPLY_KEYWORDS = 32;
export const MAX_REPLY_POLICY_CHARS = 4000;
export const MAX_COOLDOWN_SEC = 24 * 60 * 60;
const MAX_RECORD_BYTES = 64 * 1024;
const MAX_NAME_CHARS = 80;
const MAX_LABEL_CHARS = 80;
const MAX_DESCRIPTION_CHARS = 500;
const MAX_MODEL_CHARS = 256;
const MAX_PATH_CHARS = 4096;
const MAX_DOMAIN_CHARS = 253;
const MAX_REFERENCE_CHARS = 128;
const BOT_ID = /^[A-Za-z0-9_-]{22}$/;
const WORKSPACE_ID = /^[A-Za-z0-9_-]{1,32}$/;
const DOMAIN = /^(\*\.)?[a-z0-9][a-z0-9.-]*$/;
const LOCAL_APP_ID = /^[A-Za-z0-9._:-]{1,128}$/;
const CLOCK_TIME = /^([01]\d|2[0-3]):[0-5]\d$/;
const ROUTINE_ID = /^[A-Za-z0-9._:-]{1,128}$/;
const ROUTINE_FOLDERS_FILE = 'routine-folders.json';
const MAX_ROUTINE_FOLDERS = 256;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/;

export type BotWorkspaceMode = 'read' | 'write';

/** A folder of the Bot's besides its own: the owner's, bound to it. */
export interface BotWorkspace {
  /** Names the folder for as long as it is bound; the Bot's own folder is {@link DEFAULT_WORKSPACE_ID}. */
  id: string;
  /** The real path of an existing directory. */
  path: string;
  mode: BotWorkspaceMode;
  /** What the owner calls the folder; absent: its name. */
  label?: string;
  /** Epoch milliseconds. */
  addedAt: number;
}

/** What the owner gives to bind a folder. */
export interface BotWorkspaceInput {
  path: string;
  mode: BotWorkspaceMode;
  label?: string;
}

/** Whether the Bot's commands run in the engine's sandbox, and which hosts they may reach. */
export interface BotSandbox {
  enabled: boolean;
  allowedDomains: string[];
}

/** One dataset of a data source the Bot may be given. */
export interface BotDatasetScope {
  source: string;
  dataset: string;
  write: boolean;
  reportReview: boolean;
}

/** What of the owner's Local Apps and datasets the Bot may reach. */
export interface BotScope {
  localApps: string[];
  datasets: BotDatasetScope[];
}

export type BotReplyApproval = 'never' | 'uncertain' | 'always';

export interface BotQuietHours {
  /** `HH:MM`, 24-hour. */
  start: string;
  end: string;
  /** An IANA time zone name. */
  timeZone: string;
}

/** How the Bot answers in the channels it was published to. */
export interface BotAutoReply {
  /** What the owner wants the Bot to answer and what not, in their words. */
  policy: string;
  keywords: string[];
  quietHours?: BotQuietHours;
  cooldownSec: number;
  /** Below `decline` the Bot stays silent; from `run` it answers; in between it is uncertain. */
  thresholds: { decline: number; run: number };
  approval: BotReplyApproval;
  decisionProviderId?: string;
}

/** `bot.json`, version 2. */
export interface BotRecord {
  v: 2;
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
  marketOrigin?: BotMarketOrigin;
  /** The folders added to the Bot's own one, `<bot>/workspace`, which is not listed. */
  workspaces: BotWorkspace[];
  sandbox: BotSandbox;
  scope: BotScope;
  autoReply: BotAutoReply;
  /** Only the owner exists in this version. */
  audiences: Array<{ id: 'owner'; kind: 'owner' }>;
}

export function defaultSandbox(): BotSandbox {
  return { enabled: true, allowedDomains: [] };
}

export function defaultScope(): BotScope {
  return { localApps: [], datasets: [] };
}

export function defaultAutoReply(): BotAutoReply {
  return { policy: '', keywords: [], cooldownSec: 0, thresholds: { decline: 0.3, run: 0.75 }, approval: 'never' };
}

export interface BotCreateInput {
  name: string;
  description?: string;
  persona?: string;
  model?: string;
  permissionMode?: PermissionModeId;
  acceptFrom?: ReadonlyArray<string>;
  workspaces?: ReadonlyArray<BotWorkspaceInput>;
  sandbox?: BotSandbox;
  scope?: BotScope;
  autoReply?: BotAutoReply;
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
  /** Replaces the whole list. A folder that is bound already keeps its id and the time it was added; one that is left out is unbound. */
  workspaces?: ReadonlyArray<BotWorkspaceInput>;
  /** Each replaces the whole of what it names. */
  sandbox?: BotSandbox;
  scope?: BotScope;
  autoReply?: BotAutoReply;
}

export interface BotStoreOptions {
  /** The app's data folder. */
  userData: string;
  /** The user's home directory. */
  home: string;
  /** The configuration home normal sessions use. */
  defaultConfigHome: string;
  /** The brand workspace root normal sessions use (the engine's product-paths `workspace_root`). */
  defaultWorkspaceRoot: string;
  now?: () => number;
  newId?: () => string;
  /** A record could not be moved to the current version. */
  warn?: (message: string) => void;
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
  /** The brand workspace root of normal sessions (chats, Local App sources, cron runs): as given and, if it exists, as it really is. */
  defaultWorkspaceRoots: string[];
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

function checkedPath(value: unknown): string {
  if (typeof value !== 'string' || value.length === 0 || value.length > MAX_PATH_CHARS || value.includes('\0') || !isAbsolute(value)) throw new Error('invalid workspace path');
  return value;
}

function checkedMode(value: unknown): BotWorkspaceMode {
  if (value !== 'read' && value !== 'write') throw new Error('invalid workspace mode');
  return value;
}

function checkedLabel(value: unknown): string {
  return singleLine(value, 'workspace label', MAX_LABEL_CHARS);
}

/**
 * The id a folder is bound under. The same real path always has the same one, so a folder that is
 * bound again after it was unbound is the folder it was: its chats are found under it once more.
 */
export function workspaceIdOf(path: string): string {
  return `w${createHash('sha256').update(path).digest('base64url').slice(0, 10)}`;
}

/** What the owner gives to bind a folder: exactly a path and a mode, and a label if they name it. */
export function parseWorkspaceInput(value: unknown): BotWorkspaceInput {
  const entry = plainObject(value, 'workspace');
  exactKeys(entry, ['path', 'mode'], ['label'], 'workspace');
  return {
    path: checkedPath(entry['path']),
    mode: checkedMode(entry['mode']),
    ...(entry['label'] !== undefined ? { label: checkedLabel(entry['label']) } : {}),
  };
}

function parseStoredWorkspace(value: unknown): BotWorkspace {
  const entry = plainObject(value, 'workspace');
  exactKeys(entry, ['id', 'path', 'mode', 'addedAt'], ['label'], 'workspace');
  const id = entry['id'];
  if (typeof id !== 'string' || !WORKSPACE_ID.test(id) || id === DEFAULT_WORKSPACE_ID) throw new Error('invalid workspace id');
  const label = entry['label'] === undefined ? undefined : checkedLabel(entry['label']);
  if (label !== undefined && label !== entry['label']) throw new Error('invalid workspace label');
  return {
    id,
    path: checkedPath(entry['path']),
    mode: checkedMode(entry['mode']),
    ...(label !== undefined ? { label } : {}),
    addedAt: timestamp(entry['addedAt'], 'workspace time'),
  };
}

export function parseSandbox(value: unknown): BotSandbox {
  const input = plainObject(value, 'sandbox');
  exactKeys(input, ['enabled', 'allowedDomains'], [], 'sandbox');
  if (typeof input['enabled'] !== 'boolean') throw new Error('invalid sandbox');
  const domains = input['allowedDomains'];
  if (!Array.isArray(domains) || domains.length > MAX_ALLOWED_DOMAINS) throw new Error('invalid sandbox domains');
  const allowedDomains = domains.map((domain: unknown) => {
    const name = typeof domain === 'string' ? domain.trim().toLowerCase() : '';
    if (name.length === 0 || name.length > MAX_DOMAIN_CHARS || !DOMAIN.test(name)) throw new Error('invalid sandbox domain');
    return name;
  });
  if (new Set(allowedDomains).size !== allowedDomains.length) throw new Error('invalid sandbox domains');
  return { enabled: input['enabled'], allowedDomains };
}

export function parseScope(value: unknown): BotScope {
  const input = plainObject(value, 'scope');
  exactKeys(input, ['localApps', 'datasets'], [], 'scope');
  const apps = input['localApps'];
  if (!Array.isArray(apps) || apps.length > MAX_SCOPE_ENTRIES || !apps.every((id) => typeof id === 'string' && LOCAL_APP_ID.test(id))
    || new Set(apps).size !== apps.length) throw new Error('invalid scope apps');
  const datasets = input['datasets'];
  if (!Array.isArray(datasets) || datasets.length > MAX_SCOPE_ENTRIES) throw new Error('invalid scope datasets');
  const parsed = datasets.map((entry: unknown): BotDatasetScope => {
    const dataset = plainObject(entry, 'scope dataset');
    exactKeys(dataset, ['source', 'dataset', 'write', 'reportReview'], [], 'scope dataset');
    if (typeof dataset['write'] !== 'boolean' || typeof dataset['reportReview'] !== 'boolean') throw new Error('invalid scope dataset');
    return {
      source: singleLine(dataset['source'], 'scope dataset source', MAX_REFERENCE_CHARS),
      dataset: singleLine(dataset['dataset'], 'scope dataset', MAX_REFERENCE_CHARS),
      write: dataset['write'],
      reportReview: dataset['reportReview'],
    };
  });
  if (new Set(parsed.map((dataset) => `${dataset.source}\0${dataset.dataset}`)).size !== parsed.length) throw new Error('invalid scope datasets');
  return { localApps: [...apps] as string[], datasets: parsed };
}

function validTimeZone(value: unknown): value is string {
  if (typeof value !== 'string' || value.length === 0 || value.length > 64 || CONTROL.test(value)) return false;
  try { new Intl.DateTimeFormat('en-US', { timeZone: value }); return true; } catch { return false; }
}

function parseQuietHours(value: unknown): BotQuietHours {
  const input = plainObject(value, 'quiet hours');
  exactKeys(input, ['start', 'end', 'timeZone'], [], 'quiet hours');
  const { start, end, timeZone } = input;
  if (typeof start !== 'string' || typeof end !== 'string' || !CLOCK_TIME.test(start) || !CLOCK_TIME.test(end) || start === end || !validTimeZone(timeZone)) {
    throw new Error('invalid quiet hours');
  }
  return { start, end, timeZone };
}

export function parseAutoReply(value: unknown): BotAutoReply {
  const input = plainObject(value, 'autoReply');
  exactKeys(input, ['policy', 'keywords', 'cooldownSec', 'thresholds', 'approval'], ['quietHours', 'decisionProviderId'], 'autoReply');
  const policy = input['policy'];
  if (typeof policy !== 'string' || policy.length > MAX_REPLY_POLICY_CHARS || policy.includes('\0')) throw new Error('invalid reply policy');
  const keywords = input['keywords'];
  if (!Array.isArray(keywords) || keywords.length > MAX_REPLY_KEYWORDS) throw new Error('invalid reply keywords');
  const listed = keywords.map((keyword: unknown) => singleLine(keyword, 'reply keyword', 64));
  if (new Set(listed.map((keyword) => keyword.toLowerCase())).size !== listed.length) throw new Error('invalid reply keywords');
  const cooldown = input['cooldownSec'];
  if (!Number.isSafeInteger(cooldown) || (cooldown as number) < 0 || (cooldown as number) > MAX_COOLDOWN_SEC) throw new Error('invalid reply cooldown');
  const thresholds = plainObject(input['thresholds'], 'reply thresholds');
  exactKeys(thresholds, ['decline', 'run'], [], 'reply thresholds');
  const { decline, run } = thresholds;
  if (typeof decline !== 'number' || typeof run !== 'number' || !Number.isFinite(decline) || !Number.isFinite(run)
    || decline < 0 || run > 1 || decline > run) throw new Error('invalid reply thresholds');
  const approval = input['approval'];
  if (approval !== 'never' && approval !== 'uncertain' && approval !== 'always') throw new Error('invalid reply approval');
  return {
    policy,
    keywords: listed,
    ...(input['quietHours'] !== undefined ? { quietHours: parseQuietHours(input['quietHours']) } : {}),
    cooldownSec: cooldown as number,
    thresholds: { decline, run },
    approval,
    ...(input['decisionProviderId'] !== undefined ? { decisionProviderId: singleLine(input['decisionProviderId'], 'decision provider', MAX_REFERENCE_CHARS) } : {}),
  };
}

function parseAudiences(value: unknown): BotRecord['audiences'] {
  if (!Array.isArray(value) || value.length !== 1) throw new Error('invalid audiences');
  const owner = plainObject(value[0], 'audience');
  exactKeys(owner, ['id', 'kind'], [], 'audience');
  if (owner['id'] !== 'owner' || owner['kind'] !== 'owner') throw new Error('invalid audiences');
  return [{ id: 'owner', kind: 'owner' }];
}

/** The fields a record has in every version, read strictly. */
function parseCommon(input: Record<string, unknown>, expectedId: string): Omit<BotRecord, 'v' | 'workspaces' | 'sandbox' | 'scope' | 'autoReply' | 'audiences'> {
  if (input['bot_id'] !== expectedId || !isBotId(input['bot_id'])) throw new Error('bot.json belongs to a different Bot');
  const name = botName(input['name']);
  if (name !== input['name']) throw new Error('invalid name');
  const revision = input['revision'];
  if (!Number.isSafeInteger(revision) || (revision as number) < 1) throw new Error('invalid revision');
  const description = botDescription(input['description']);
  if (description !== input['description']) throw new Error('invalid description');
  const acceptFrom = input['accept_from'] === undefined ? [] : acceptFromList(input['accept_from'], expectedId);
  const remote = input['remote'] === undefined ? false : botRemote(input['remote']);
  return {
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
    ...(input['marketOrigin'] !== undefined ? { marketOrigin: parseMarketOrigin(input['marketOrigin']) } : {}),
  };
}

const COMMON_REQUIRED = ['v', 'bot_id', 'name', 'description', 'created_at', 'updated_at', 'revision', 'workspaces', 'audiences'] as const;
const COMMON_OPTIONAL = ['model', 'permission_mode', 'accept_from', 'remote', 'marketOrigin'] as const;

/** Read `bot.json` strictly: the one version, the exact fields, nothing else. */
export function parseRecord(text: string, expectedId: string): BotRecord {
  let parsed: unknown;
  try { parsed = JSON.parse(text); } catch { throw new Error('bot.json is not valid JSON'); }
  const input = plainObject(parsed, 'bot.json');
  if (input['v'] !== BOT_RECORD_VERSION) throw new Error('bot.json has an unsupported version');
  exactKeys(input, [...COMMON_REQUIRED, 'sandbox', 'scope', 'autoReply'], COMMON_OPTIONAL, 'bot.json');
  const common = parseCommon(input, expectedId);
  if (!Array.isArray(input['workspaces']) || input['workspaces'].length > MAX_BOT_WORKSPACES) throw new Error('invalid workspaces');
  const workspaces = input['workspaces'].map(parseStoredWorkspace);
  if (new Set(workspaces.map((workspace) => workspace.path)).size !== workspaces.length
    || new Set(workspaces.map((workspace) => workspace.id)).size !== workspaces.length) throw new Error('invalid workspaces');
  return {
    v: BOT_RECORD_VERSION,
    ...common,
    workspaces,
    sandbox: parseSandbox(input['sandbox']),
    scope: parseScope(input['scope']),
    autoReply: parseAutoReply(input['autoReply']),
    audiences: parseAudiences(input['audiences']),
  };
}

/**
 * The one place a version 1 record is read. It becomes a version 2 record: its folders get ids and the
 * time the Bot was made as the time they were added, and the sandbox, scope and replies start as
 * they start in a new Bot. A text that is not a version 1 record is none of this function's business.
 */
export function upgradeRecord(text: string, expectedId: string): BotRecord | undefined {
  let parsed: unknown;
  try { parsed = JSON.parse(text); } catch { return undefined; }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed) || (parsed as Record<string, unknown>)['v'] !== 1) return undefined;
  const input = parsed as Record<string, unknown>;
  exactKeys(input, COMMON_REQUIRED, COMMON_OPTIONAL, 'bot.json');
  const common = parseCommon(input, expectedId);
  const folders = input['workspaces'];
  if (!Array.isArray(folders) || folders.length > MAX_BOT_WORKSPACES) throw new Error('invalid workspaces');
  const workspaces = folders.map((entry: unknown): BotWorkspace => {
    const folder = plainObject(entry, 'workspace');
    exactKeys(folder, ['path', 'mode'], [], 'workspace');
    const path = checkedPath(folder['path']);
    return { id: workspaceIdOf(path), path, mode: checkedMode(folder['mode']), addedAt: common.created_at };
  });
  if (new Set(workspaces.map((workspace) => workspace.path)).size !== workspaces.length) throw new Error('invalid workspaces');
  return {
    v: BOT_RECORD_VERSION,
    ...common,
    workspaces,
    sandbox: defaultSandbox(),
    scope: defaultScope(),
    autoReply: defaultAutoReply(),
    audiences: parseAudiences(input['audiences']),
  };
}

function parseMarketOrigin(value: unknown): BotMarketOrigin {
  const origin = plainObject(value, 'market origin');
  exactKeys(origin, ['market','accountId','listingId','releaseId','digest','installIntent'], [], 'market origin');
  for (const key of ['accountId','listingId','releaseId']) if (typeof origin[key] !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(origin[key] as string)) throw new Error('invalid market origin');
  if (typeof origin.digest !== 'string' || !/^[a-f0-9]{64}$/.test(origin.digest) || typeof origin.installIntent !== 'string' || !/^[a-f0-9]{64}$/.test(origin.installIntent) || typeof origin.market !== 'string') throw new Error('invalid market origin');
  const url = new URL(origin.market);
  if (url.username || url.password || url.search || url.hash || (url.protocol !== 'https:' && !['localhost','127.0.0.1'].includes(url.hostname))) throw new Error('invalid market origin');
  return origin as unknown as BotMarketOrigin;
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
    const spellings = (path: string): string[] => {
      const given = [resolve(path)];
      try {
        const real = realpathSync.native(given[0]!);
        if (!given.includes(real)) given.push(real);
      } catch { /* A folder that does not exist yet has one spelling. */ }
      return given;
    };
    const defaultConfigHomes = spellings(options.defaultConfigHome);
    const defaultWorkspaceRoots = spellings(options.defaultWorkspaceRoot);
    this.locations = {
      userData,
      root: realDirectory(root),
      trashRoot: join(userData, 'bots-trash'),
      home,
      defaultConfigHomes,
      defaultWorkspaceRoots,
      defaultGlobalConfigs: [join(home, '.lingxi.json')],
    };
    this.now = options.now ?? Date.now;
    this.newId = options.newId ?? (() => randomBytes(16).toString('base64url'));
    this.upgradeRecords(options.warn);
  }

  get root(): string { return this.locations.root; }

  /**
   * Bring every record on disk to the current version, once, when the store opens: a version 1 record is
   * rewritten whole as version 2, atomically, and nothing reads version 1 after that. A record that is
   * current, damaged or of another version is left as it is; reading it says what is wrong.
   */
  private upgradeRecords(warn: ((message: string) => void) | undefined): void {
    for (const botId of readdirSync(this.root).sort()) {
      if (!isBotId(botId)) continue;
      const path = this.recordPath(botId);
      try {
        if (!existsSync(path) || statSync(path).size > MAX_RECORD_BYTES) continue;
        const upgraded = upgradeRecord(readFileSync(path, 'utf8'), botId);
        if (upgraded !== undefined) writeFileAtomic(path, serialize(upgraded));
      } catch (error) {
        warn?.(`Bot ${botId}: bot.json could not be moved to version ${BOT_RECORD_VERSION}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
  }

  botDir(botId: string): string { return join(this.root, this.checkedId(botId)); }
  recordPath(botId: string): string { return join(this.botDir(botId), 'bot.json'); }
  homeDir(botId: string): string { return join(this.botDir(botId), 'home'); }
  workspaceDir(botId: string): string { return join(this.botDir(botId), 'workspace'); }
  /** `<bot>/tasks`: the tasks other Bots handed this one, a file each. They go to the trash with the Bot. */
  tasksDir(botId: string): string { return join(this.botDir(botId), 'tasks'); }

  /** `<home>/projects/<project dir>`: where the engine keeps the transcripts of the Bot's own folder, and the Bot's memory. */
  transcriptsDir(botId: string): string {
    return this.transcriptsDirOf(botId, this.workspaceDir(botId));
  }

  /** Where the engine keeps the transcripts of sessions that ran in a folder: one project folder in the Bot's home for each. */
  transcriptsDirOf(botId: string, projectPath: string): string {
    return join(this.homeDir(botId), 'projects', projectDirName(projectPath));
  }

  /**
   * Every folder the Bot works in, its own first: `<bot>/workspace` is always there and always writable,
   * and the owner's are listed after it.
   */
  allWorkspaces(record: BotRecord): BotWorkspace[] {
    return [{ id: DEFAULT_WORKSPACE_ID, path: this.workspaceDir(record.bot_id), mode: 'write', addedAt: record.created_at }, ...record.workspaces];
  }

  /** A folder of the Bot by its id, the Bot's own one included. */
  workspaceById(record: BotRecord, workspaceId: string): BotWorkspace | undefined {
    return this.allWorkspaces(record).find((workspace) => workspace.id === workspaceId);
  }

  /** Private exact-source record; Bot sessions never enter the owner's space store. */
  sessionSource(botId: string, sessionId: string): { path: string; mode: 'chat' | 'code' } | undefined {
    this.get(botId);
    return this.readSessionSource(botId, sessionId);
  }

  private readSessionSource(botId: string, sessionId: string): { path: string; mode: 'chat' | 'code' } | undefined {
    if (!isSessionId(sessionId)) throw new Error('invalid session id');
    const file = join(this.botDir(botId), 'session-sources', `${sessionId.toLowerCase()}.json`);
    let value: unknown;
    try {
      const stat = lstatSync(file);
      if (!stat.isFile() || stat.size > MAX_RECORD_BYTES) throw new Error('invalid Bot session source');
      value = JSON.parse(readFileSync(file, 'utf8'));
    } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; }
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('invalid Bot session source');
    const row = value as Record<string, unknown>;
    if (typeof row['path'] !== 'string' || row['path'].length > 32_768 || !isAbsolute(row['path']) || row['path'].includes('\0')
      || basename(row['path']).toLowerCase() !== `${sessionId.toLowerCase()}.jsonl`
      || (row['mode'] !== 'chat' && row['mode'] !== 'code')) throw new Error('invalid Bot session source');
    return { path: row['path'], mode: row['mode'] };
  }

  rememberSessionSource(botId: string, sessionId: string, source: { path: string; mode: 'chat' | 'code' }): void {
    const previous = this.sessionSource(botId, sessionId);
    if (!isAbsolute(source.path) || source.path.length > 32_768 || source.path.includes('\0') || basename(source.path).toLowerCase() !== `${sessionId.toLowerCase()}.jsonl`
      || (source.mode !== 'chat' && source.mode !== 'code')) throw new Error('invalid Bot session source');
    if (previous && (previous.path !== source.path || previous.mode !== source.mode)) throw new Error('The Bot session already has a different saved source.');
    if (!previous) writeFileAtomic(join(this.botDir(botId), 'session-sources', `${sessionId.toLowerCase()}.json`), JSON.stringify(source));
  }

  /**
   * The Bot that recorded a session as its own and kept its transcript in the project folder of
   * `projectPath`. This is how a session of a folder the owner bound, which has the same working
   * directory as the owner's own sessions there, is known for a Bot's after the app restarted.
   */
  botIdOfRecordedSession(sessionId: string, projectPath: string): string | undefined {
    if (!isSessionId(sessionId) || typeof projectPath !== 'string' || !isAbsolute(projectPath)) return undefined;
    for (const botId of readdirSync(this.root)) {
      if (!isBotId(botId)) continue;
      try {
        const source = this.readSessionSource(botId, sessionId);
        if (source !== undefined && dirname(source.path) === this.transcriptsDirOf(botId, projectPath)) return botId;
      } catch { /* A damaged record says nothing. */ }
    }
    return undefined;
  }

  /**
   * The folder of `<home>/projects` (by name) that holds a session's transcript: where the Bot recorded
   * it, else wherever a transcript of that name is.
   */
  private transcriptFolder(botId: string, sessionId: string): string | undefined {
    const projects = join(this.homeDir(botId), 'projects');
    try {
      const source = this.readSessionSource(botId, sessionId);
      if (source !== undefined && dirname(dirname(source.path)) === projects) return basename(dirname(source.path));
    } catch { /* A damaged record says nothing. */ }
    let folders: string[];
    try { folders = readdirSync(projects); } catch { return undefined; }
    return folders.find((folder) => existsSync(join(projects, folder, `${sessionId}.jsonl`)));
  }

  /** The folder of the Bot that a session ran in, if it is one the Bot works in now. */
  workspaceOfTranscript(record: BotRecord, sessionId: string): BotWorkspace | undefined {
    if (!isSessionId(sessionId)) return undefined;
    const folder = this.transcriptFolder(record.bot_id, sessionId);
    return folder === undefined ? undefined : this.allWorkspaces(record).find((workspace) => projectDirName(workspace.path) === folder);
  }

  /**
   * Which folder each routine of the Bot runs in, by routine id. A routine that is not listed runs in
   * the Bot's own folder. The scheduler's tasks are the engine's and have nowhere to say this.
   */
  routineFolders(botId: string): Record<string, string> {
    this.get(botId);
    const file = join(this.botDir(botId), ROUTINE_FOLDERS_FILE);
    let value: unknown;
    try {
      const stat = lstatSync(file);
      if (!stat.isFile() || stat.size > MAX_RECORD_BYTES) throw new Error('invalid routine folders');
      value = JSON.parse(readFileSync(file, 'utf8'));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return {};
      throw error;
    }
    const input = plainObject(value, 'routine folders');
    exactKeys(input, ['v', 'folders'], [], 'routine folders');
    const folders = plainObject(input['folders'], 'routine folders');
    if (input['v'] !== 1 || Object.keys(folders).length > MAX_ROUTINE_FOLDERS) throw new Error('invalid routine folders');
    const result: Record<string, string> = {};
    for (const [routineId, workspaceId] of Object.entries(folders)) {
      if (!ROUTINE_ID.test(routineId) || typeof workspaceId !== 'string' || !WORKSPACE_ID.test(workspaceId) || workspaceId === DEFAULT_WORKSPACE_ID) throw new Error('invalid routine folders');
      result[routineId] = workspaceId;
    }
    return result;
  }

  /** The folder a routine runs in; `undefined` puts it back in the Bot's own folder. */
  setRoutineFolder(botId: string, routineId: string, workspaceId: string | undefined): void {
    if (!ROUTINE_ID.test(routineId)) throw new Error('invalid routine id');
    if (workspaceId !== undefined && (!WORKSPACE_ID.test(workspaceId) || workspaceId === DEFAULT_WORKSPACE_ID)) throw new Error('invalid workspace id');
    const folders = this.routineFolders(botId);
    if (workspaceId === undefined) delete folders[routineId];
    else folders[routineId] = workspaceId;
    if (Object.keys(folders).length > MAX_ROUTINE_FOLDERS) throw new Error('A Bot has too many routines with a folder of their own.');
    const file = join(this.botDir(botId), ROUTINE_FOLDERS_FILE);
    if (Object.keys(folders).length === 0) { rmSync(file, { force: true }); return; }
    writeFileAtomic(file, `${JSON.stringify({ v: 1, folders }, null, 2)}\n`);
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
   * The Bot whose home holds the transcript of a session, in whichever of its folders the session ran, if
   * any. The id must be a session's: it becomes part of a file name.
   */
  botIdOfTranscript(sessionId: string): string | undefined {
    if (!isSessionId(sessionId)) return undefined;
    for (const botId of readdirSync(this.root)) {
      if (isBotId(botId) && this.transcriptFolder(botId, sessionId) !== undefined) return botId;
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
  /** Atomic market materialization: the record is committed after all public content. */
  createFromMarket(input: BotCreateInput, origin: BotMarketOrigin, prepare: (record: BotRecord) => void): BotRecord {
    const checked = parseMarketOrigin(origin);
    const existing = this.list().find(bot => bot.marketOrigin?.installIntent === checked.installIntent && bot.marketOrigin.accountId === checked.accountId && bot.marketOrigin.market === checked.market);
    if (existing) return existing;
    return this.create(input, record => { record.marketOrigin = checked; prepare(record); });
  }

  create(input: BotCreateInput, prepare?: (record: BotRecord) => void): BotRecord {
    const name = botName(input.name);
    const description = input.description === undefined ? '' : botDescription(input.description);
    const model = input.model === undefined ? undefined : botModel(input.model);
    const permissionMode = input.permissionMode === undefined ? undefined : botPermissionMode(input.permissionMode);
    const acceptFrom = this.acceptFrom(input.acceptFrom ?? [], undefined, []);
    const now = this.now();
    const workspaces = this.boundFolders(input.workspaces ?? [], [], now);
    const sandbox = input.sandbox === undefined ? defaultSandbox() : parseSandbox(input.sandbox);
    const scope = input.scope === undefined ? defaultScope() : parseScope(input.scope);
    const autoReply = input.autoReply === undefined ? defaultAutoReply() : parseAutoReply(input.autoReply);
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
        sandbox,
        scope,
        autoReply,
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
  /**
   * A market Bot moved to another release of the listing it was installed from. Its files are already in place; the record is
   * what says which release it is. `name` and `description` are given only when the owner never wrote their own.
   */
  upgradeFromMarket(botId: string, expectedRevision: number, release: { releaseId: string; digest: string; name?: string; description?: string }): BotRecord {
    if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 1) throw new Error('invalid revision');
    const current = this.get(botId);
    if (current.revision !== expectedRevision) throw new BotConflictError();
    if (!current.marketOrigin) throw new Error('This Bot was not installed from the market');
    const next: BotRecord = {
      ...current,
      ...(release.name !== undefined ? { name: botName(release.name) } : {}),
      ...(release.description !== undefined ? { description: botDescription(release.description) } : {}),
      marketOrigin: parseMarketOrigin({ ...current.marketOrigin, releaseId: release.releaseId, digest: release.digest }),
      revision: current.revision + 1,
      updated_at: Math.max(this.now(), current.updated_at),
    };
    writeFileAtomic(this.recordPath(botId), serialize(next));
    return next;
  }

  update(botId: string, expectedRevision: number, patch: BotPatch, prepare?: (record: BotRecord) => void): BotRecord {
    if (!Number.isSafeInteger(expectedRevision) || expectedRevision < 1) throw new Error('invalid revision');
    const current = this.get(botId);
    if (current.revision !== expectedRevision) throw new BotConflictError();
    const next: BotRecord = {
      ...current,
      ...(patch.name !== undefined ? { name: botName(patch.name) } : {}),
      ...(patch.description !== undefined ? { description: botDescription(patch.description) } : {}),
      ...(patch.workspaces !== undefined ? { workspaces: this.boundFolders(patch.workspaces, current.workspaces, this.now()) } : {}),
      ...(patch.sandbox !== undefined ? { sandbox: parseSandbox(patch.sandbox) } : {}),
      ...(patch.scope !== undefined ? { scope: parseScope(patch.scope) } : {}),
      ...(patch.autoReply !== undefined ? { autoReply: parseAutoReply(patch.autoReply) } : {}),
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
      && (['workspaces', 'sandbox', 'scope', 'autoReply'] as const).every((key) => JSON.stringify(next[key]) === JSON.stringify(current[key]));
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
   * included), or the configuration home and workspace root of normal sessions.
   */
  workspace(path: unknown, mode: unknown): { path: string; mode: BotWorkspaceMode } {
    const entry = { path: checkedPath(path), mode: checkedMode(mode) };
    let real: string;
    try { real = realDirectory(entry.path); } catch { throw new Error('The folder does not exist or is not a directory.'); }
    const { userData, root, home, defaultConfigHomes, defaultWorkspaceRoots } = this.locations;
    if (real === parse(real).root) throw new Error('A Bot cannot work in the filesystem root.');
    if (isInside(home, real)) throw new Error('A Bot cannot work in your home folder or in a folder that contains it.');
    if (isInside(real, userData) || isInside(root, real)) throw new Error("A Bot cannot work in the app's own data folder.");
    if (defaultConfigHomes.some((configHome) => isInside(real, configHome) || isInside(configHome, real))) {
      throw new Error('A Bot cannot work in the LingXi configuration folder.');
    }
    if (defaultWorkspaceRoots.some((workspaceRoot) => isInside(real, workspaceRoot) || isInside(workspaceRoot, real))) {
      throw new Error('A Bot cannot work in the LingXi workspace folder.');
    }
    return { path: real, mode: entry.mode };
  }

  /** The list of Bots that may delegate here, sorted: valid, once each, existing (or listed already), and not the Bot itself. */
  private acceptFrom(ids: ReadonlyArray<string>, selfId: string | undefined, listed: readonly string[]): string[] {
    const list = acceptFromList(ids, selfId);
    if (list.some((id) => !listed.includes(id) && !existsSync(this.recordPath(id)))) throw new Error('accept_from names a Bot that does not exist');
    return list.sort();
  }

  /**
   * The folders a list names, as the Bot has them. One that the Bot has bound already keeps its id and
   * the time it was added; the others are bound as of `now`.
   */
  private boundFolders(entries: ReadonlyArray<BotWorkspaceInput>, existing: readonly BotWorkspace[], now: number): BotWorkspace[] {
    if (!Array.isArray(entries) || entries.length > MAX_BOT_WORKSPACES) throw new Error('invalid workspaces');
    const bound = entries.map((entry): BotWorkspace => {
      const input = parseWorkspaceInput(entry);
      const checked = this.workspace(input.path, input.mode);
      const earlier = existing.find((workspace) => workspace.path === checked.path);
      return {
        id: earlier?.id ?? workspaceIdOf(checked.path),
        path: checked.path,
        mode: checked.mode,
        ...(input.label !== undefined ? { label: input.label } : {}),
        addedAt: earlier?.addedAt ?? now,
      };
    });
    if (new Set(bound.map((workspace) => workspace.path)).size !== bound.length) throw new Error('The same folder is bound twice.');
    return bound;
  }

  /** Write the record that follows `current`, once `prepare` has had its say. */
  private commit(current: BotRecord, changes: Partial<BotRecord>, prepare?: (record: BotRecord) => void): BotRecord {
    const next: BotRecord = { ...current, ...changes, revision: current.revision + 1, updated_at: Math.max(this.now(), current.updated_at) };
    prepare?.(next);
    writeFileAtomic(this.recordPath(current.bot_id), serialize(next));
    return next;
  }

  /** Bind one more folder to a Bot, whatever revision it is at. A folder it has bound already is refused. */
  addWorkspace(botId: string, entry: BotWorkspaceInput, prepare?: (record: BotRecord) => void): BotRecord {
    const current = this.get(botId);
    const input = parseWorkspaceInput(entry);
    const checked = this.workspace(input.path, input.mode);
    if (current.workspaces.some((workspace) => workspace.path === checked.path)) throw new Error("The folder is one of the Bot's folders already.");
    if (current.workspaces.length >= MAX_BOT_WORKSPACES) throw new Error(`A Bot can work in ${MAX_BOT_WORKSPACES} folders besides its own, and no more.`);
    const added: BotWorkspace = {
      id: workspaceIdOf(checked.path),
      path: checked.path,
      mode: checked.mode,
      ...(input.label !== undefined ? { label: input.label } : {}),
      addedAt: this.now(),
    };
    return this.commit(current, { workspaces: [...current.workspaces, added] }, prepare);
  }

  /** Change the mode or the label of a folder that was bound: `label: null` removes the label. Nothing changes if nothing would. */
  changeWorkspace(botId: string, workspaceId: string, change: { mode?: BotWorkspaceMode; label?: string | null }, prepare?: (record: BotRecord) => void): BotRecord {
    const current = this.get(botId);
    if (workspaceId === DEFAULT_WORKSPACE_ID) throw new Error("The Bot's own folder cannot be changed.");
    const index = current.workspaces.findIndex((workspace) => workspace.id === workspaceId);
    if (index < 0) throw new Error('The Bot has no such folder.');
    const before = current.workspaces[index]!;
    const label = change.label === undefined ? before.label : change.label === null ? undefined : checkedLabel(change.label);
    const after: BotWorkspace = {
      id: before.id,
      path: before.path,
      mode: change.mode === undefined ? before.mode : checkedMode(change.mode),
      ...(label !== undefined ? { label } : {}),
      addedAt: before.addedAt,
    };
    if (JSON.stringify(after) === JSON.stringify(before)) return current;
    return this.commit(current, { workspaces: current.workspaces.map((workspace, at) => at === index ? after : workspace) }, prepare);
  }

  /** Unbind a folder. The Bot's own one cannot be unbound. What the Bot did there stays in its home. */
  removeWorkspace(botId: string, workspaceId: string, prepare?: (record: BotRecord) => void): BotRecord {
    const current = this.get(botId);
    if (workspaceId === DEFAULT_WORKSPACE_ID) throw new Error("The Bot's own folder cannot be removed.");
    if (!current.workspaces.some((workspace) => workspace.id === workspaceId)) throw new Error('The Bot has no such folder.');
    return this.commit(current, { workspaces: current.workspaces.filter((workspace) => workspace.id !== workspaceId) }, prepare);
  }

  private checkedId(botId: string): string {
    if (!isBotId(botId)) throw new Error('invalid Bot id');
    return botId;
  }
}

/** A folder bound earlier that still is what it was: a real directory at the same real path, still allowed. */
export function stillBound(store: BotStore, workspace: Pick<BotWorkspace, 'path' | 'mode'>): boolean {
  try {
    if (lstatSync(workspace.path).isSymbolicLink()) return false;
    return store.workspace(workspace.path, workspace.mode).path === workspace.path;
  } catch {
    return false;
  }
}
