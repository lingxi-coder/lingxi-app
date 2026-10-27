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

export interface BotDraft {
  name: string;
  description: string;
  /** The text of the Bot's persona file. */
  persona: string;
  /** Empty: the model new chats use. */
  model: string;
  /** Empty: the engine's default, which is the mode that asks before it acts. */
  permissionMode: PermissionModeId | '';
  workspaces: BotWorkspaceUi[];
}

export type BotDraftProblem =
  | 'name_required' | 'name_too_long' | 'name_invalid'
  | 'description_too_long'
  | 'persona_too_long'
  | 'folders_too_many' | 'folders_duplicate';

export type BotDraftProblems = Partial<Record<'name' | 'description' | 'persona' | 'folders', BotDraftProblem>>;

// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f]/;

/** Full access is the one default mode that is asked about again before it is taken, as it is in a chat. */
export function permissionModeNeedsConfirmation(mode: PermissionModeId | ''): boolean {
  return mode === 'bypassPermissions';
}

export function emptyBotDraft(): BotDraft {
  return { name: '', description: '', persona: '', model: '', permissionMode: '', workspaces: [] };
}

export function draftFromBot(bot: BotUi, persona: string): BotDraft {
  return {
    name: bot.name,
    description: bot.description,
    persona,
    model: bot.model ?? '',
    // Asking is what the default is, so a Bot that names it is shown as having none.
    permissionMode: bot.permissionMode === undefined || bot.permissionMode === 'default' ? '' : bot.permissionMode,
    workspaces: bot.workspaces.map((workspace) => ({ ...workspace })),
  };
}

export function botDraftProblems(draft: BotDraft): BotDraftProblems {
  const problems: BotDraftProblems = {};
  const name = draft.name.trim();
  if (name.length === 0) problems.name = 'name_required';
  else if (name.length > BOT_NAME_MAX) problems.name = 'name_too_long';
  else if (CONTROL.test(name)) problems.name = 'name_invalid';
  if (draft.description.trim().length > BOT_DESCRIPTION_MAX) problems.description = 'description_too_long';
  if (new TextEncoder().encode(draft.persona).length > BOT_PERSONA_MAX_BYTES) problems.persona = 'persona_too_long';
  if (draft.workspaces.length > BOT_FOLDERS_MAX) problems.folders = 'folders_too_many';
  else if (new Set(draft.workspaces.map((workspace) => workspace.path)).size !== draft.workspaces.length) problems.folders = 'folders_duplicate';
  return problems;
}

export function hasBotDraftProblems(problems: BotDraftProblems): boolean {
  return Object.keys(problems).length > 0;
}

/** What a new Bot is made from. Blank optional parts are left out, so that the Bot gets the defaults. */
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

/** The persona is kept apart from the record, so its change is told separately from the patch. */
export function botChanges(bot: BotUi, persona: string, draft: BotDraft): { patch: BotUpdateRequest; personaChanged: boolean } {
  const patch: BotUpdateRequest = {};
  if (draft.name.trim() !== bot.name) patch.name = draft.name.trim();
  if (draft.description.trim() !== bot.description) patch.description = draft.description.trim();
  if (draft.model !== (bot.model ?? '')) patch.model = draft.model || null;
  if ((draft.permissionMode || 'default') !== (bot.permissionMode ?? 'default')) patch.permissionMode = draft.permissionMode || null;
  if (!sameFolders(draft.workspaces, bot.workspaces)) patch.workspaces = draft.workspaces.map((workspace) => ({ ...workspace }));
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
export async function saveBot(api: Pick<BotsApi, 'update' | 'persona'>, bot: BotUi, persona: string, draft: BotDraft): Promise<BotSave> {
  if (hasBotDraftProblems(botDraftProblems(draft))) return { ok: false, conflict: false, message: '' };
  const { patch, personaChanged } = botChanges(bot, persona, draft);
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
