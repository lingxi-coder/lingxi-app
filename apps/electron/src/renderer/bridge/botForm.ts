/**
 * The Bot form without the screen: what a draft may hold, what is wrong with
 * it, and the requests that making or changing a Bot sends. The limits are the
 * ones the main process enforces; checking them here only spares a round trip.
 */

import type { PermissionModeId } from '@lingxi/bridge-client';
import {
  BOT_CONFLICT_MESSAGE,
  type BotCreateRequest,
  type BotUi,
  type BotUpdateRequest,
  type BotWorkspaceMode,
  type BotWorkspaceUi,
  type BotsApi,
} from '../../shared/bots';

export const BOT_NAME_MAX = 80;
export const BOT_DESCRIPTION_MAX = 500;
export const BOT_PERSONA_MAX_BYTES = 64 * 1024;
export const BOT_FOLDERS_MAX = 16;
/** How many Bots may be allowed to hand tasks to one. */
export const BOT_ACCEPT_MAX = 64;

export interface BotDraft {
  name: string;
  description: string;
  /** The text of the Bot's persona file. */
  persona: string;
  /** Empty: the model new chats use. */
  model: string;
  /** Empty: leave the Bot's settings in charge. `default` explicitly asks for approval. */
  permissionMode: PermissionModeId | '';
  workspaces: BotWorkspaceUi[];
  /** The ids of the Bots that may hand this one tasks, as saved; a Bot that was deleted since may still be among them. */
  acceptFrom: string[];
  /** Whether the owner's approved phones and browsers can reach this Bot. A new Bot is made with this off. */
  remote: boolean;
}

export type BotDraftProblem =
  | 'name_required' | 'name_too_long' | 'name_invalid'
  | 'description_too_long'
  | 'persona_too_long'
  | 'folders_too_many' | 'folders_duplicate'
  | 'accept_too_many';

export type BotDraftProblems = Partial<Record<'name' | 'description' | 'persona' | 'folders' | 'accept', BotDraftProblem>>;

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
  return { name: '', description: '', persona: '', model: '', permissionMode: 'default', workspaces: [], acceptFrom: [], remote: false };
}

export function draftFromBot(bot: BotUi, persona: string): BotDraft {
  return {
    name: bot.name,
    description: bot.description,
    persona,
    model: bot.model ?? '',
    permissionMode: bot.permissionMode ?? '',
    workspaces: bot.workspaces.map((workspace) => ({ ...workspace })),
    acceptFrom: [...(bot.acceptFrom ?? [])],
    remote: bot.remote === true,
  };
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
  // Ids of Bots that are gone are dropped when the list is saved, so they are not counted.
  if (present(draft.acceptFrom, peers).length > BOT_ACCEPT_MAX) problems.accept = 'accept_too_many';
  return problems;
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
    ...(draft.workspaces.length > 0 ? { workspaces: draft.workspaces.map((workspace) => ({ ...workspace })) } : {}),
  };
}

function sameFolders(left: readonly BotWorkspaceUi[], right: readonly BotWorkspaceUi[]): boolean {
  return left.length === right.length && left.every((folder, index) => folder.path === right[index]!.path && folder.mode === right[index]!.mode);
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
  if (!sameFolders(draft.workspaces, bot.workspaces)) patch.workspaces = draft.workspaces.map((workspace) => ({ ...workspace }));
  if (draft.remote !== (bot.remote === true)) patch.remote = draft.remote;
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
export function withFolder(workspaces: readonly BotWorkspaceUi[], path: string, mode: BotWorkspaceMode = 'read'): BotWorkspaceUi[] {
  return workspaces.some((folder) => folder.path === path) ? workspaces.map((folder) => ({ ...folder })) : [...workspaces.map((folder) => ({ ...folder })), { path, mode }];
}
