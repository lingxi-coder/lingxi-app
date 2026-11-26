/**
 * The Bot form without the screen: what a draft may hold, what is wrong with
 * it, and the requests that making or changing a Bot sends. The limits are the
 * ones the main process enforces; checking them here only spares a round trip.
 */

import type { PermissionModeId } from '@lingxi/bridge-client';
import {
  BOT_ALLOWED_DOMAINS_MAX,
  BOT_CLOCK_TIME,
  BOT_CONFLICT_MESSAGE,
  BOT_REPLY_COOLDOWN_SEC_MAX,
  BOT_REPLY_KEYWORDS_MAX,
  BOT_REPLY_KEYWORD_CHARS_MAX,
  BOT_REPLY_POLICY_CHARS_MAX,
  BOT_REPLY_THRESHOLDS_DEFAULT,
  BOT_SANDBOX_UNAVAILABLE_MESSAGE,
  allowedDomain,
  validTimeZone,
  type BotAutoReplyUi,
  type BotCreateRequest,
  type BotDatasetScopeUi,
  type BotQuietHoursUi,
  type BotReplyApprovalUi,
  type BotSandboxUi,
  type BotScopeUi,
  type BotUi,
  type BotUpdateRequest,
  type BotWorkspaceInputUi,
  type BotWorkspaceMode,
  type BotsApi,
} from '../../shared/bots';
import { BOT_FOLDER_LABEL_MAX } from './botFolders';

export const BOT_NAME_MAX = 80;
export const BOT_DESCRIPTION_MAX = 500;
export const BOT_PERSONA_MAX_BYTES = 64 * 1024;
export const BOT_FOLDERS_MAX = 16;
/** How many Bots may be allowed to hand tasks to one. */
export const BOT_ACCEPT_MAX = 64;
/** How many Local Apps, and how many datasets, a Bot's scope may name. */
export const BOT_SCOPE_MAX = 64;

export interface BotDraft {
  name: string;
  description: string;
  /** The text of the Bot's persona file. */
  persona: string;
  /** Empty: the model new chats use. */
  model: string;
  /** Empty: leave the Bot's settings in charge. `default` explicitly asks for approval. */
  permissionMode: PermissionModeId | '';
  /** The folders besides the Bot's own: where they are, how far the Bot may go in them, and what the owner calls them. */
  workspaces: BotWorkspaceInputUi[];
  /** The ids of the Bots that may hand this one tasks, as saved; a Bot that was deleted since may still be among them. */
  acceptFrom: string[];
  /** Whether the owner's approved phones and browsers can reach this Bot. A new Bot is made with this off. */
  remote: boolean;
  /** Whether the Bot's shell commands run in the sandbox, and which hosts they may reach in it. A new Bot is made with the sandbox on and no host. */
  sandbox: BotSandboxUi;
  /** The Local Apps and datasets of the owner's that the Bot may be given, at the most. A new Bot is made with none. */
  scope: BotScopeUi;
  /** When the Bot answers on its own in the chats of the Teams it is published to. */
  autoReply: BotReplyDraft;
}

/**
 * What the owner edits of how the Bot answers on its own. The cooldown stays the text that was typed until it is sent, so that a
 * half-typed number is no number; the times of the quiet hours are what the time fields hold, empty while cleared.
 */
export interface BotReplyDraft {
  /** In the owner's words; blank: the Bot judges by itself whether a reply is wanted. */
  policy: string;
  /** One word or phrase each, in the order they were added. */
  keywords: string[];
  /** Absent: the Bot may answer at any hour. */
  quietHours?: BotQuietHoursUi;
  /** Seconds, as typed. */
  cooldown: string;
  /** Below `decline` the Bot stays silent; from `run` it answers; in between it answers if it still finds a reply is wanted. */
  thresholds: { decline: number; run: number };
  /** Whether the owner reads a reply before it is posted: never, only one the Bot was unsure it should send, or every one. Held replies are approved in the Team's chat. */
  approval: BotReplyApprovalUi;
}

export type BotDraftProblem =
  | 'name_required' | 'name_too_long' | 'name_invalid'
  | 'description_too_long'
  | 'persona_too_long'
  | 'folders_too_many' | 'folders_duplicate' | 'folders_label_too_long'
  | 'accept_too_many'
  | 'sandbox_too_many'
  | 'scope_too_many'
  | 'reply_policy_too_long' | 'reply_keywords_too_many' | 'reply_quiet_invalid' | 'reply_cooldown_invalid' | 'reply_thresholds_invalid';

export type BotDraftProblems = Partial<Record<'name' | 'description' | 'persona' | 'folders' | 'accept' | 'sandbox' | 'scope'
  | 'replyPolicy' | 'replyKeywords' | 'replyQuiet' | 'replyCooldown' | 'replyThresholds', BotDraftProblem>>;

/**
 * The ids of the owner's other Bots, which the accept list is chosen among.
 * Unknown (absent) until the list of Bots has been read and has this Bot in
 * it: nothing is then known to be gone, so the accept list is left alone.
 */
export type BotPeers = ReadonlySet<string> | undefined;

export function botPeers(bots: readonly Pick<BotUi, 'botId'>[], botId: string): BotPeers {
  return bots.some((bot) => bot.botId === botId) ? new Set(bots.filter((bot) => bot.botId !== botId).map((bot) => bot.botId)) : undefined;
}

/** The ids of an accept list that are Bots to choose among. Where the Bots are not known, all of them. */
function present(ids: readonly string[], peers: BotPeers): string[] {
  return peers === undefined ? [...ids] : ids.filter((id) => peers.has(id));
}

function sameIds(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((id) => right.includes(id));
}

// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/;

/** Full access is the one default mode that is asked about again before it is taken, as it is in a chat. */
export function permissionModeNeedsConfirmation(mode: PermissionModeId | ''): boolean {
  return mode === 'bypassPermissions';
}

export function emptyBotDraft(): BotDraft {
  return {
    name: '', description: '', persona: '', model: '', permissionMode: 'default', workspaces: [], acceptFrom: [], remote: false,
    sandbox: { enabled: true, allowedDomains: [] }, scope: { localApps: [], datasets: [] }, autoReply: emptyReplyDraft(),
  };
}

export function draftFromBot(bot: BotUi, persona: string): BotDraft {
  return {
    name: bot.name,
    description: bot.description,
    persona,
    model: bot.model ?? '',
    permissionMode: bot.permissionMode ?? '',
    workspaces: bot.workspaces.map(folderInput),
    acceptFrom: [...(bot.acceptFrom ?? [])],
    remote: bot.remote === true,
    sandbox: { enabled: bot.sandbox.enabled, allowedDomains: [...bot.sandbox.allowedDomains] },
    scope: scopeInput(bot.scope),
    autoReply: replyDraft(bot.autoReply),
  };
}

/**
 * The draft once folders were bound to the Bot somewhere else (the owner allowed a request of the Bot's): what is being
 * edited stays as it is, and the folders that are new are added to the list, so that saving does not take them away again.
 */
export function draftWithNewFolders(draft: BotDraft, before: Pick<BotUi, 'workspaces'>, after: Pick<BotUi, 'workspaces'>): BotDraft {
  const known = new Set([...before.workspaces, ...draft.workspaces].map((folder) => folder.path));
  const added = after.workspaces.filter((folder) => !known.has(folder.path)).map(folderInput);
  return added.length === 0 ? draft : { ...draft, workspaces: [...draft.workspaces, ...added] };
}

export function botDraftProblems(draft: BotDraft, peers?: BotPeers): BotDraftProblems {
  const problems: BotDraftProblems = {};
  const name = draft.name.trim();
  if (name.length === 0) problems.name = 'name_required';
  else if (name.length > BOT_NAME_MAX) problems.name = 'name_too_long';
  else if (CONTROL.test(name)) problems.name = 'name_invalid';
  if (draft.description.trim().length > BOT_DESCRIPTION_MAX) problems.description = 'description_too_long';
  if (new TextEncoder().encode(draft.persona).length > BOT_PERSONA_MAX_BYTES) problems.persona = 'persona_too_long';
  if (draft.workspaces.length > BOT_FOLDERS_MAX) problems.folders = 'folders_too_many';
  else if (new Set(draft.workspaces.map((workspace) => workspace.path)).size !== draft.workspaces.length) problems.folders = 'folders_duplicate';
  else if (draft.workspaces.some((workspace) => (workspace.label?.trim().length ?? 0) > BOT_FOLDER_LABEL_MAX)) problems.folders = 'folders_label_too_long';
  // Ids of Bots that are gone are dropped when the list is saved, so they are not counted.
  if (present(draft.acceptFrom, peers).length > BOT_ACCEPT_MAX) problems.accept = 'accept_too_many';
  if (draft.sandbox.allowedDomains.length > BOT_ALLOWED_DOMAINS_MAX) problems.sandbox = 'sandbox_too_many';
  if (draft.scope.localApps.length > BOT_SCOPE_MAX || draft.scope.datasets.length > BOT_SCOPE_MAX) problems.scope = 'scope_too_many';
  const { autoReply } = draft;
  if (autoReply.policy.trim().length > BOT_REPLY_POLICY_CHARS_MAX) problems.replyPolicy = 'reply_policy_too_long';
  if (autoReply.keywords.length > BOT_REPLY_KEYWORDS_MAX) problems.replyKeywords = 'reply_keywords_too_many';
  if (autoReply.quietHours !== undefined && !validQuietHours(autoReply.quietHours)) problems.replyQuiet = 'reply_quiet_invalid';
  if (cooldownSeconds(autoReply.cooldown) === undefined) problems.replyCooldown = 'reply_cooldown_invalid';
  const { decline, run } = autoReply.thresholds;
  if (!Number.isFinite(decline) || !Number.isFinite(run) || decline < 0 || run > 1 || decline > run) problems.replyThresholds = 'reply_thresholds_invalid';
  return problems;
}

/** The times of quiet hours are two different times of day in a time zone that is known here; they may run past midnight. */
function validQuietHours(quiet: BotQuietHoursUi): boolean {
  return BOT_CLOCK_TIME.test(quiet.start) && BOT_CLOCK_TIME.test(quiet.end) && quiet.start !== quiet.end && validTimeZone(quiet.timeZone);
}

/** What the cooldown text says, in seconds: a whole number from 0 to a day. Absent when it says anything else. */
export function cooldownSeconds(text: string): number | undefined {
  const typed = text.trim();
  if (!/^\d{1,6}$/.test(typed)) return undefined;
  const seconds = Number(typed);
  return seconds <= BOT_REPLY_COOLDOWN_SEC_MAX ? seconds : undefined;
}

export function emptyReplyDraft(): BotReplyDraft {
  return { policy: '', keywords: [], cooldown: '0', thresholds: { ...BOT_REPLY_THRESHOLDS_DEFAULT }, approval: 'never' };
}

function replyDraft(reply: BotAutoReplyUi): BotReplyDraft {
  return {
    policy: reply.policy,
    keywords: [...reply.keywords],
    ...(reply.quietHours ? { quietHours: { ...reply.quietHours } } : {}),
    cooldown: String(reply.cooldownSec),
    thresholds: { ...reply.thresholds },
    approval: reply.approval,
  };
}

/** The settings as they are sent: the policy without the blanks around it, and the cooldown as the number it says (none is sent while it says none). */
function replyInput(reply: BotReplyDraft): BotAutoReplyUi {
  return {
    policy: reply.policy.trim(),
    keywords: [...reply.keywords],
    ...(reply.quietHours ? { quietHours: { start: reply.quietHours.start, end: reply.quietHours.end, timeZone: reply.quietHours.timeZone } } : {}),
    cooldownSec: cooldownSeconds(reply.cooldown) ?? 0,
    thresholds: { decline: reply.thresholds.decline, run: reply.thresholds.run },
    approval: reply.approval,
  };
}

/** The same settings, as far as saving them is concerned: blanks around the policy and a cooldown written as 030 or 30 are no change. */
export function sameReply(left: BotReplyDraft, right: BotReplyDraft): boolean {
  const cooldown = (reply: BotReplyDraft) => cooldownSeconds(reply.cooldown) ?? reply.cooldown.trim();
  const quiet = (reply: BotReplyDraft) => reply.quietHours === undefined ? '' : `${reply.quietHours.start}\u0000${reply.quietHours.end}\u0000${reply.quietHours.timeZone}`;
  return left.policy.trim() === right.policy.trim() && left.keywords.length === right.keywords.length && left.keywords.every((keyword, index) => keyword === right.keywords[index])
    && quiet(left) === quiet(right) && cooldown(left) === cooldown(right)
    && left.thresholds.decline === right.thresholds.decline && left.thresholds.run === right.thresholds.run && left.approval === right.approval;
}

/** What adding a keyword to the ones that make the Bot answer came to: the list with it, or why it was not added. */
export type BotKeywordAdd = { ok: true; keywords: string[] } | { ok: false; problem: 'invalid' | 'duplicate' | 'too_many' };

/** Add what the owner typed to the keywords. It is kept the way the main process keeps it: one trimmed line, and no two that differ only in case. */
export function withReplyKeyword(keywords: readonly string[], text: string): BotKeywordAdd {
  const keyword = text.trim();
  if (keyword.length === 0 || keyword.length > BOT_REPLY_KEYWORD_CHARS_MAX || CONTROL.test(keyword)) return { ok: false, problem: 'invalid' };
  if (keywords.some((known) => known.toLowerCase() === keyword.toLowerCase())) return { ok: false, problem: 'duplicate' };
  if (keywords.length >= BOT_REPLY_KEYWORDS_MAX) return { ok: false, problem: 'too_many' };
  return { ok: true, keywords: [...keywords, keyword] };
}

/** The time zone this computer is in, which is the one quiet hours are set in. */
export function computerTimeZone(): string {
  try {
    const zone = new Intl.DateTimeFormat().resolvedOptions().timeZone;
    if (validTimeZone(zone)) return zone;
  } catch { /* the zone is unknown: UTC says so */ }
  return 'UTC';
}

/** Quiet hours as they start when the owner turns them on: the night, in the zone of this computer. */
export function startingQuietHours(timeZone: string = computerTimeZone()): BotQuietHoursUi {
  return { start: '22:00', end: '07:00', timeZone };
}

/** The settings with quiet hours off or, with `quiet`, on as given. */
export function withQuietHours(reply: BotReplyDraft, quiet: BotQuietHoursUi | undefined): BotReplyDraft {
  const { quietHours: _off, ...rest } = reply;
  return quiet === undefined ? rest : { ...rest, quietHours: quiet };
}

const roundedToHundredths = (value: number) => Math.round(Math.min(1, Math.max(0, value)) * 100) / 100;

/** Move the value below which the Bot stays silent. It does not pass the one from which it answers. */
export function withDeclineThreshold(reply: BotReplyDraft, value: number): BotReplyDraft {
  return { ...reply, thresholds: { ...reply.thresholds, decline: Math.min(roundedToHundredths(value), reply.thresholds.run) } };
}

/** Move the value from which the Bot answers. It does not go under the one below which the Bot stays silent. */
export function withRunThreshold(reply: BotReplyDraft, value: number): BotReplyDraft {
  return { ...reply, thresholds: { ...reply.thresholds, run: Math.max(roundedToHundredths(value), reply.thresholds.decline) } };
}

/** What adding a host to the list that the sandbox allows came to: the list with it, or why it was not added. */
export type BotHostAdd = { ok: true; domains: string[] } | { ok: false; problem: 'invalid' | 'duplicate' | 'too_many' };

/** Add what the owner typed to the hosts the sandbox allows. It is kept the way the main process keeps it: lower case, and a name that is no more than a name. */
export function withAllowedHost(domains: readonly string[], text: string): BotHostAdd {
  const host = allowedDomain(text);
  if (host === undefined) return { ok: false, problem: 'invalid' };
  if (domains.includes(host)) return { ok: false, problem: 'duplicate' };
  if (domains.length >= BOT_ALLOWED_DOMAINS_MAX) return { ok: false, problem: 'too_many' };
  return { ok: true, domains: [...domains, host] };
}

/** The sandbox as it is sent. */
function sandboxInput(sandbox: BotSandboxUi): BotSandboxUi {
  return { enabled: sandbox.enabled, allowedDomains: [...sandbox.allowedDomains] };
}

function sameSandbox(left: BotSandboxUi, right: BotSandboxUi): boolean {
  return left.enabled === right.enabled && left.allowedDomains.length === right.allowedDomains.length && left.allowedDomains.every((domain, index) => domain === right.allowedDomains[index]);
}

/** A copy of the scope with what is sent of it: the apps, and the four things that are said of each dataset. */
function scopeInput(scope: BotScopeUi): BotScopeUi {
  return { localApps: [...scope.localApps], datasets: scope.datasets.map((entry) => ({ source: entry.source, dataset: entry.dataset, write: entry.write, reportReview: entry.reportReview })) };
}

/** The same Local Apps, and the same datasets with the same things allowed in each, in whatever order. */
export function sameScope(left: BotScopeUi, right: BotScopeUi): boolean {
  const within = (from: readonly BotDatasetScopeUi[], among: readonly BotDatasetScopeUi[]) => from.every((entry) => among.some((other) =>
    other.source === entry.source && other.dataset === entry.dataset && other.write === entry.write && other.reportReview === entry.reportReview));
  return sameIds(left.localApps, right.localApps) && left.datasets.length === right.datasets.length
    && within(left.datasets, right.datasets) && within(right.datasets, left.datasets);
}

export function hasBotDraftProblems(problems: BotDraftProblems): boolean {
  return Object.keys(problems).length > 0;
}

/** What a new Bot is made from. Blank optional parts are left out, so that the Bot gets the defaults; a new Bot accepts tasks from no one and is out of reach of remote devices. */
export function botCreateRequest(draft: BotDraft): BotCreateRequest {
  return {
    name: draft.name.trim(),
    ...(draft.description.trim() ? { description: draft.description.trim() } : {}),
    ...(draft.persona.trim() ? { persona: draft.persona } : {}),
    ...(draft.model ? { model: draft.model } : {}),
    ...(draft.permissionMode ? { permissionMode: draft.permissionMode } : {}),
    ...(draft.workspaces.length > 0 ? { workspaces: draft.workspaces.map(folderInput) } : {}),
    ...(sameSandbox(draft.sandbox, emptyBotDraft().sandbox) ? {} : { sandbox: sandboxInput(draft.sandbox) }),
    ...(sameReply(draft.autoReply, emptyReplyDraft()) ? {} : { autoReply: replyInput(draft.autoReply) }),
  };
}

/** A folder as it is sent: a path and a mode, and a name only if the owner gave one. */
function folderInput(folder: BotWorkspaceInputUi): BotWorkspaceInputUi {
  const label = folder.label?.trim();
  return { path: folder.path, mode: folder.mode, ...(label ? { label } : {}) };
}

function sameFolders(left: readonly BotWorkspaceInputUi[], right: readonly BotWorkspaceInputUi[]): boolean {
  return left.length === right.length && left.every((folder, index) => {
    const other = right[index]!;
    return folder.path === other.path && folder.mode === other.mode && (folder.label?.trim() || undefined) === (other.label?.trim() || undefined);
  });
}

/**
 * The persona is kept apart from the record, so its change is told separately from the patch.
 * The accept list is part of the patch only once the Bots it is chosen among are known, and then
 * without the ids of Bots that are gone, which are no change of the owner's to save.
 */
export function botChanges(bot: BotUi, persona: string, draft: BotDraft, peers?: BotPeers): { patch: BotUpdateRequest; personaChanged: boolean } {
  const patch: BotUpdateRequest = {};
  if (draft.name.trim() !== bot.name) patch.name = draft.name.trim();
  if (draft.description.trim() !== bot.description) patch.description = draft.description.trim();
  if (draft.model !== (bot.model ?? '')) patch.model = draft.model || null;
  if (draft.permissionMode !== (bot.permissionMode ?? '')) patch.permissionMode = draft.permissionMode || null;
  if (!sameFolders(draft.workspaces, bot.workspaces)) patch.workspaces = draft.workspaces.map(folderInput);
  if (draft.remote !== (bot.remote === true)) patch.remote = draft.remote;
  if (!sameSandbox(draft.sandbox, bot.sandbox)) patch.sandbox = sandboxInput(draft.sandbox);
  if (!sameScope(draft.scope, bot.scope)) patch.scope = scopeInput(draft.scope);
  if (!sameReply(draft.autoReply, replyDraft(bot.autoReply))) patch.autoReply = replyInput(draft.autoReply);
  if (peers !== undefined) {
    const accepted = present(draft.acceptFrom, peers);
    if (!sameIds(accepted, present(bot.acceptFrom ?? [], peers))) patch.acceptFrom = accepted;
  }
  return { patch, personaChanged: draft.persona !== persona };
}

export function isEmptyPatch(patch: BotUpdateRequest): boolean {
  return Object.keys(patch).length === 0;
}

/** The text of an error from the main process, without the wrapper the IPC layer puts around it. */
export function ipcErrorMessage(error: unknown): string {
  const raw = error instanceof Error ? error.message : typeof error === 'string' ? error : '';
  return raw.replace(/^Error invoking remote method '[^']*':\s*/, '').replace(/^[A-Za-z]*Error:\s*/, '').trim();
}

/** An error crosses the IPC boundary as its message alone, so a conflict is known by its text. */
export function isBotConflict(error: unknown): boolean {
  return ipcErrorMessage(error).includes(BOT_CONFLICT_MESSAGE);
}

/** An error crosses the IPC boundary as its message alone, so a Bot refused for want of a sandbox is known by its text. */
export function isBotSandboxUnavailable(error: unknown): boolean {
  return ipcErrorMessage(error).includes(BOT_SANDBOX_UNAVAILABLE_MESSAGE);
}

export type BotSave =
  | { ok: true; bot: BotUi; persona: string }
  | { ok: false; conflict: true }
  /** `saved` is set when the record was written and only the persona was not. */
  | { ok: false; conflict: false; message: string; saved?: BotUi };

/**
 * Make a Bot. Nothing is sent while the draft has problems; a failure is
 * returned as words for the owner rather than thrown.
 */
export async function createBot(api: Pick<BotsApi, 'create'>, draft: BotDraft): Promise<Exclude<BotSave, { conflict: true }>> {
  if (hasBotDraftProblems(botDraftProblems(draft))) return { ok: false, conflict: false, message: '' };
  try {
    const bot = await api.create(botCreateRequest(draft));
    return { ok: true, bot, persona: draft.persona };
  } catch (error) {
    return { ok: false, conflict: false, message: ipcErrorMessage(error) };
  }
}

/**
 * Save what changed in a Bot that was read at `bot.revision` with `persona`.
 * A Bot that changed elsewhere meanwhile is a conflict: nothing is overwritten,
 * and the owner is asked to reload. The persona is written after the record, so
 * that a conflict never leaves it half saved.
 */
export async function saveBot(api: Pick<BotsApi, 'update' | 'persona'>, bot: BotUi, persona: string, draft: BotDraft, peers?: BotPeers): Promise<BotSave> {
  if (hasBotDraftProblems(botDraftProblems(draft, peers))) return { ok: false, conflict: false, message: '' };
  const { patch, personaChanged } = botChanges(bot, persona, draft, peers);
  let saved = bot;
  try {
    if (!isEmptyPatch(patch)) saved = await api.update(bot.botId, bot.revision, patch);
  } catch (error) {
    if (isBotConflict(error)) return { ok: false, conflict: true };
    return { ok: false, conflict: false, message: ipcErrorMessage(error) };
  }
  try {
    if (personaChanged) await api.persona.set(bot.botId, draft.persona);
  } catch (error) {
    return { ok: false, conflict: false, message: ipcErrorMessage(error), ...(saved !== bot ? { saved } : {}) };
  }
  return { ok: true, bot: saved, persona: draft.persona };
}

/** Most recently used first; a Bot that has had no chat counts from when it was made. */
export function sortBots(bots: readonly BotUi[]): BotUi[] {
  return [...bots].sort((left, right) => (right.lastActivityAt ?? right.createdAt) - (left.lastActivityAt ?? left.createdAt) || left.name.localeCompare(right.name));
}

/** Add a folder the owner picked. One that is already there is kept as it is. */
export function withFolder(workspaces: readonly BotWorkspaceInputUi[], path: string, mode: BotWorkspaceMode = 'read'): BotWorkspaceInputUi[] {
  return workspaces.some((folder) => folder.path === path) ? workspaces.map((folder) => ({ ...folder })) : [...workspaces.map((folder) => ({ ...folder })), { path, mode }];
}
