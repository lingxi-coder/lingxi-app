/**
 * The `settings.json` of a Bot's configuration home, and with it what the Bot
 * may touch. LingXi generates it; the engine reads it as the user settings of
 * the Bot's sessions, whichever of the Bot's folders they work in. A session
 * in a folder the owner bound reads that folder's own project settings as well
 * (its hooks, MCP servers and skills are the owner's, as in any of their
 * sessions there), and the files that could loosen these settings are among
 * those the Bot may not edit, in the Bot's own folder and in the bound ones.
 *
 * What is generated:
 *  - `permissions.additionalDirectories`: every folder the Bot works in, its
 *    own `<bot>/workspace` included, so that a session in any of them reaches
 *    the others.
 *  - `permissions.deny`: no `Read` (which the engine also applies to Grep and
 *    Glob) and no `Edit` (Edit, Write, NotebookEdit) in the other Bots, the other
 *    private data of the app, or the configuration home of normal sessions; no
 *    `Edit` in a folder bound read-only; no `Edit` in the files that decide what
 *    the Bot may do (`bot.json`, its settings and persona, `.mcp.json`, the project
 *    settings of a bound folder, and the descriptor that tells its engine where
 *    the Local Apps are) or in the tasks other Bots handed it.
 *  - `permissions.allow`: reading and writing the Bot's own memory, which the
 *    engine does not exempt from the working-directory rule, the tools of the
 *    Bot router, which hand work to other Bots (what that work may do is decided
 *    by the permissions of the Bot that gets it, not by this), and, once the Bot
 *    was given Local Apps, the Local Apps' MCP server.
 *  - `autoMemoryDirectory`: the Bot's memory folder, so that a session in any
 *    folder remembers the same things. This is the user layer of the engine's
 *    settings, the one every session of the Bot reads whether or not its folder
 *    is trusted; a trusted bound folder whose own project settings name another
 *    memory folder would take precedence over it, which is the owner's choice
 *    to make in their own folder.
 *  - `sandbox`: on by default, refusing to start without it, with the same
 *    private places unreadable to the commands of the Bash tool, which no
 *    permission rule reaches, and the hosts those commands may reach (`allowedDomains`
 *    of the Bot). The sandbox is off in the engine unless the settings turn it on.
 *    A Bot whose owner switched the sandbox off gets `enabled: false` and none of
 *    the rest: its commands are then confined by nothing but the permission rules,
 *    which are all there in either case.
 *  - `model` and `permissions.defaultMode`, when the Bot has them. The Bot's
 *    mode is the one its sessions start in; the owner's own last choice never
 *    reaches a Bot.
 *
 * Everything else in the file stays as it is. The entries LingXi wrote last
 * time are remembered in a file beside `settings.json`, so that regenerating
 * removes exactly those, adds the current ones, and leaves what anyone else put
 * there; regenerating twice changes nothing.
 */

import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

import type { PermissionModeId } from '@lingxi/bridge-client';

import { isPermissionModeId } from '../host-utils.js';
import { LOCAL_APP_MCP_DESCRIPTOR, LOCAL_APP_MCP_SERVER_NAME } from '../local-apps/mcpGateway.js';
import { writeFileAtomic } from './files.js';
import { routerToolRules } from './routerTools.js';
import { stillBound, type BotRecord, type BotStore, type BotWorkspace } from './store.js';

/** Beside `settings.json`: the entries LingXi generated into it. */
export const MANAGED_FILE = '.lingxi-bot-managed.json';

export interface ManagedSettings {
  model?: string;
  defaultMode?: PermissionModeId;
  /** Where the Bot's memory is, for a session in whichever folder. */
  autoMemoryDirectory?: string;
  /** Whether the sandbox is on. When it is not, `denyRead` and `allowedDomains` are empty. */
  sandbox: boolean;
  additionalDirectories: string[];
  allow: string[];
  deny: string[];
  denyRead: string[];
  /** The hosts the commands of the sandbox may reach. */
  allowedDomains: string[];
}

/** Escape what a gitignore-style pattern treats specially: `\ * ? [ ] { }`, and a space that ends the pattern. */
function globLiteral(path: string): string {
  return path.replace(/[\\*?[\]{}]/g, '\\$&').replace(/ $/, '\\ ');
}

/**
 * A permission rule for an absolute path: `//` roots it at `/`, and for a
 * directory `/**` takes in everything below it. The path is escaped twice, once
 * for the pattern and once for the `Tool(...)` syntax around it.
 */
export function pathRule(tool: 'Read' | 'Edit', absolutePath: string, directory = true): string {
  if (!absolutePath.startsWith('/')) throw new Error('Bot isolation needs an absolute POSIX path');
  const pattern = `//${globLiteral(absolutePath.slice(1))}${directory ? '/**' : ''}`;
  return `${tool}(${pattern.replace(/[\\()]/g, '\\$&')})`;
}

/** Every place this Bot must not see: its siblings, the app's other data, the default configuration home and workspace root. */
function isolatedPaths(store: BotStore, botId: string): string[] {
  const { userData, root, defaultConfigHomes, defaultWorkspaceRoots, defaultGlobalConfigs } = store.locations;
  const paths = new Set<string>(store.otherEntries(botId));
  // Every entry of the data folder but the Bots folder, whatever put it there:
  // the bridge's lockfiles, the settings, the account, the logs, the trash.
  for (const name of readdirSync(userData)) {
    const path = join(userData, name);
    if (path !== root) paths.add(path);
  }
  for (const path of [...defaultConfigHomes, ...defaultWorkspaceRoots, ...defaultGlobalConfigs]) paths.add(path);
  return [...paths].sort();
}

/**
 * The rule that lets a Bot use the Local Apps it was given without being asked each time. It names the
 * server, not its tools: the engine's allow rules match a tool by its whole name or a server as a whole,
 * and the names of the tools are made when an application is running. What the server lists to the Bot
 * is decided by the Bot's own credential, so the Bot is allowed what it was given and nothing else.
 */
export const LOCAL_APP_RULE = `mcp__${LOCAL_APP_MCP_SERVER_NAME}`;

/** What decides, in a folder the owner bound, what a session there may do next time: the Bot may not edit it. */
function projectConfigFiles(folder: string): string[] {
  return [join(folder, '.lingxi', 'settings.json'), join(folder, '.lingxi', 'settings.local.json'), join(folder, '.mcp.json')];
}

export function buildManagedSettings(store: BotStore, record: BotRecord, workspaces: readonly BotWorkspace[]): ManagedSettings {
  const botId = record.bot_id;
  const botDir = store.botDir(botId);
  const home = store.homeDir(botId);
  const workspace = store.workspaceDir(botId);
  const isolated = isolatedPaths(store, botId);
  const memory = store.memoryDir(botId);
  const sandboxed = record.sandbox.enabled;
  const deny = [
    ...isolated.flatMap((path) => [pathRule('Read', path), pathRule('Edit', path)]),
    ...workspaces.filter((bound) => bound.mode === 'read').map((bound) => pathRule('Edit', bound.path)),
    // The files whose contents decide what a session may do next time.
    ...[
      join(botDir, 'bot.json'),
      join(home, 'LINGXI.md'), join(home, 'settings.json'), join(home, 'settings.local.json'), join(home, '.lingxi.json'), join(home, MANAGED_FILE),
      join(home, LOCAL_APP_MCP_DESCRIPTOR),
      ...projectConfigFiles(workspace),
      ...workspaces.flatMap((bound) => projectConfigFiles(bound.path)),
    ].map((path) => pathRule('Edit', path, false)),
    pathRule('Edit', store.tasksDir(botId)),
  ];
  return {
    ...(record.model !== undefined ? { model: record.model } : {}),
    ...(record.permission_mode !== undefined ? { defaultMode: record.permission_mode } : {}),
    autoMemoryDirectory: memory,
    sandbox: sandboxed,
    additionalDirectories: [workspace, ...workspaces.map((bound) => bound.path)],
    allow: [pathRule('Read', memory), pathRule('Edit', memory), ...routerToolRules(), ...(record.scope.localApps.length > 0 ? [LOCAL_APP_RULE] : [])],
    deny,
    denyRead: sandboxed ? isolated : [],
    allowedDomains: sandboxed ? [...record.sandbox.allowedDomains] : [],
  };
}

function plainObject(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw new Error(`${what} must be an object`);
  return value as Record<string, unknown>;
}

function stringList(value: unknown, what: string): string[] | undefined {
  if (value === undefined) return undefined;
  if (!Array.isArray(value) || value.some((entry) => typeof entry !== 'string')) throw new Error(`${what} must be a list of strings`);
  return value as string[];
}

function readText(path: string): string | undefined {
  try { return readFileSync(path, 'utf8'); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw error;
  }
}

function writeIfChanged(path: string, text: string): void {
  if (readText(path) !== text) writeFileAtomic(path, text);
}

function parseManaged(text: string, path: string): ManagedSettings {
  try {
    const input = plainObject(JSON.parse(text), 'managed settings');
    if (input['v'] !== 1 || Object.keys(input).some((key) => !['v', 'model', 'defaultMode', 'autoMemoryDirectory', 'sandbox', 'additionalDirectories', 'allow', 'deny', 'denyRead', 'allowedDomains'].includes(key))) {
      throw new Error('unsupported');
    }
    const lists = (['additionalDirectories', 'allow', 'deny', 'denyRead'] as const).map((key) => stringList(input[key], key));
    const allowedDomains = stringList(input['allowedDomains'], 'allowedDomains');
    if (lists.some((list) => list === undefined) || (input['model'] !== undefined && typeof input['model'] !== 'string')
      || (input['autoMemoryDirectory'] !== undefined && typeof input['autoMemoryDirectory'] !== 'string')
      || (input['sandbox'] !== undefined && typeof input['sandbox'] !== 'boolean')
      || (input['defaultMode'] !== undefined && !isPermissionModeId(input['defaultMode']))) throw new Error('invalid');
    return {
      ...(typeof input['model'] === 'string' ? { model: input['model'] } : {}),
      ...(isPermissionModeId(input['defaultMode']) ? { defaultMode: input['defaultMode'] } : {}),
      ...(typeof input['autoMemoryDirectory'] === 'string' ? { autoMemoryDirectory: input['autoMemoryDirectory'] } : {}),
      // A file from before the sandbox could be switched off has neither: it was written with the sandbox on and no hosts.
      sandbox: input['sandbox'] !== false,
      additionalDirectories: lists[0]!, allow: lists[1]!, deny: lists[2]!, denyRead: lists[3]!, allowedDomains: allowedDomains ?? [],
    };
  } catch {
    // Guessing which entries are ours could leave a removed grant in place.
    throw new Error(`${path} is unreadable. Remove it to let LingXi write the Bot's settings again.`);
  }
}

function serializeManaged(managed: ManagedSettings): string {
  return `${JSON.stringify({ v: 1, ...managed }, null, 2)}\n`;
}

function union(left: ManagedSettings, right: ManagedSettings): ManagedSettings {
  const merge = (a: string[], b: string[]): string[] => [...new Set([...a, ...b])];
  const model = right.model ?? left.model;
  const defaultMode = right.defaultMode ?? left.defaultMode;
  const autoMemoryDirectory = right.autoMemoryDirectory ?? left.autoMemoryDirectory;
  return {
    ...(model !== undefined ? { model } : {}),
    ...(defaultMode !== undefined ? { defaultMode } : {}),
    ...(autoMemoryDirectory !== undefined ? { autoMemoryDirectory } : {}),
    sandbox: right.sandbox,
    additionalDirectories: merge(left.additionalDirectories, right.additionalDirectories),
    allow: merge(left.allow, right.allow),
    deny: merge(left.deny, right.deny),
    denyRead: merge(left.denyRead, right.denyRead),
    allowedDomains: merge(left.allowedDomains, right.allowedDomains),
  };
}

function objectField(parent: Record<string, unknown>, key: string, what: string): Record<string, unknown> {
  const value = parent[key];
  if (value === undefined) {
    const created: Record<string, unknown> = {};
    parent[key] = created;
    return created;
  }
  return plainObject(value, `settings.json: ${what}`);
}

/** The list at `parent[key]` without what we wrote before, followed by what we write now. */
function mergeList(parent: Record<string, unknown>, key: string, what: string, previous: readonly string[] | undefined, next: readonly string[]): void {
  const current = stringList(parent[key], `settings.json: ${what}`);
  const foreign = (current ?? []).filter((entry) => !previous?.includes(entry) && !next.includes(entry));
  const merged = [...foreign, ...next];
  if (merged.length > 0 || current !== undefined) parent[key] = merged;
}

/** The list at `sandbox[group][key]`, in a group that is not made only to be left without it. */
function mergeSandboxList(sandbox: Record<string, unknown>, group: 'filesystem' | 'network', key: string, previous: readonly string[] | undefined, next: readonly string[]): void {
  if (next.length === 0 && sandbox[group] === undefined) return;
  mergeList(objectField(sandbox, group, `sandbox.${group}`), key, `sandbox.${group}.${key}`, previous, next);
}

export function applyManagedSettings(
  existing: Record<string, unknown>,
  previous: ManagedSettings | undefined,
  next: ManagedSettings,
): Record<string, unknown> {
  const settings = structuredClone(existing);
  if (next.model !== undefined) settings['model'] = next.model;
  else if (previous?.model !== undefined && settings['model'] === previous.model) delete settings['model'];
  if (next.autoMemoryDirectory !== undefined) settings['autoMemoryDirectory'] = next.autoMemoryDirectory;
  else if (previous?.autoMemoryDirectory !== undefined && settings['autoMemoryDirectory'] === previous.autoMemoryDirectory) delete settings['autoMemoryDirectory'];
  const permissions = objectField(settings, 'permissions', 'permissions');
  if (next.defaultMode !== undefined) permissions['defaultMode'] = next.defaultMode;
  else if (previous?.defaultMode !== undefined && permissions['defaultMode'] === previous.defaultMode) delete permissions['defaultMode'];
  mergeList(permissions, 'additionalDirectories', 'permissions.additionalDirectories', previous?.additionalDirectories, next.additionalDirectories);
  mergeList(permissions, 'allow', 'permissions.allow', previous?.allow, next.allow);
  mergeList(permissions, 'deny', 'permissions.deny', previous?.deny, next.deny);
  // Whether the sandbox is on is the Bot's to say, whatever the file said; refusing to start without it goes with it.
  const sandbox = objectField(settings, 'sandbox', 'sandbox');
  sandbox['enabled'] = next.sandbox;
  if (next.sandbox) sandbox['failIfUnavailable'] = true;
  else delete sandbox['failIfUnavailable'];
  mergeSandboxList(sandbox, 'filesystem', 'denyRead', previous?.denyRead, next.denyRead);
  mergeSandboxList(sandbox, 'network', 'allowedDomains', previous?.allowedDomains, next.allowedDomains);
  return settings;
}

/**
 * Write the Bot's `settings.json` for its record as it is now. Folders bound
 * earlier that are no longer what they were (gone, replaced, or now somewhere a
 * Bot may not work) are left out and returned. Throws, leaving the files as they
 * were, if `settings.json` is not a JSON object or the entries it holds are not
 * of the shape the engine needs.
 */
export function writeBotSettings(store: BotStore, record: BotRecord): { bound: BotWorkspace[]; skipped: BotWorkspace[] } {
  const bound = record.workspaces.filter((workspace) => stillBound(store, workspace));
  const skipped = record.workspaces.filter((workspace) => !bound.includes(workspace));
  const next = buildManagedSettings(store, record, bound);
  const settingsPath = join(store.homeDir(record.bot_id), 'settings.json');
  const managedPath = join(store.homeDir(record.bot_id), MANAGED_FILE);
  const managedText = readText(managedPath);
  const previous = managedText === undefined ? undefined : parseManaged(managedText, managedPath);
  const settingsText = readText(settingsPath);
  let existing: Record<string, unknown> = {};
  if (settingsText !== undefined && settingsText.trim() !== '') {
    let parsed: unknown;
    try { parsed = JSON.parse(settingsText); } catch { throw new Error(`${settingsPath} is not valid JSON`); }
    existing = plainObject(parsed, settingsPath);
  }
  const merged = applyManagedSettings(existing, previous, next);
  // If this stops half way, the file still names the union of both
  // generations, and the next run removes whichever was left behind.
  writeIfChanged(managedPath, serializeManaged(previous === undefined ? next : union(previous, next)));
  writeIfChanged(settingsPath, `${JSON.stringify(merged, null, 2)}\n`);
  writeIfChanged(managedPath, serializeManaged(next));
  return { bound, skipped };
}

/** Make every Bot's settings current, for when a Bot appears or goes: the others must now keep out of it, or stop doing so. */
export function writeAllBotSettings(store: BotStore, warn: (message: string) => void, except?: string): void {
  for (const entry of store.entries()) {
    if (entry.botId === except || !existsSync(store.homeDir(entry.botId))) continue;
    if (entry.record === undefined) {
      warn(`Bot ${entry.botId} has a damaged record: ${entry.error?.message ?? 'unreadable'}`);
      continue;
    }
    try {
      for (const workspace of writeBotSettings(store, entry.record).skipped) warn(`Bot ${entry.botId}: ${workspace.path} can no longer be used`);
    } catch (error) {
      warn(`Bot ${entry.botId}: settings were not updated: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
}
