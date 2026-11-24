/**
 * How a Bot's folders are told apart on the owner's screen: the name a folder
 * is shown under, and which folder each of the Bot's chats worked in. The Bot's
 * own folder is always there and is not one of the folders the owner gives.
 */

import { BOT_DEFAULT_WORKSPACE_ID, type BotSessionRowUi, type BotUi, type BotWorkspaceInputUi } from '../../shared/bots';
import { tr } from '../i18n/locale';

/** The longest name the owner may give a folder; the main process enforces the same. */
export const BOT_FOLDER_LABEL_MAX = 80;

/** The last part of a path, whichever way the system writes its separators. */
export function folderBaseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, '');
  const name = trimmed.slice(Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\')) + 1);
  return name === '' ? path : name;
}

/** What a folder is called on screen: the name the owner gave it, else the folder's own name. */
export function folderTitle(folder: Pick<BotWorkspaceInputUi, 'path' | 'label'>): string {
  const label = folder.label?.trim();
  return label ? label : folderBaseName(folder.path);
}

/** What the Bot's own folder is called on screen. */
export function ownFolderTitle(): string {
  return tr('bots_folder_own');
}

/** The folder a chat works in, from where it works: a folder of the Bot's own list, else the Bot's own folder. */
export function workspaceIdAtPath(bot: Pick<BotUi, 'workspaces'>, projectPath: string | undefined): string {
  return bot.workspaces.find((folder) => folder.path === projectPath)?.id ?? BOT_DEFAULT_WORKSPACE_ID;
}

/** A group of the Bot's chats in the sidebar: the chats of one folder, or those of folders the Bot no longer has. */
export type ChatGroupKind = 'own' | 'folder' | 'removed';

export interface ChatGroup<Row extends Pick<BotSessionRowUi, 'workspaceId' | 'removedPath'>> {
  kind: ChatGroupKind;
  /** Names the group within the Bot. */
  key: string;
  title: string;
  /** The folder's path, for a group of one folder the Bot has. */
  path?: string;
  /** What starting a chat in the group's folder is asked with: absent for the Bot's own folder, which is where a chat starts by default. */
  workspaceId?: string;
  chats: Row[];
}

/**
 * The chats of a Bot, grouped by the folder they worked in: the Bot's own folder first, then the folders the owner
 * gave in their order, and last the chats of folders the Bot no longer has. A Bot with no folders but its own has one
 * group, which the sidebar draws without a header.
 */
export function chatGroups<Row extends Pick<BotSessionRowUi, 'workspaceId' | 'removedPath'>>(bot: Pick<BotUi, 'workspaces'>, chats: readonly Row[]): { groups: ChatGroup<Row>[]; headed: boolean } {
  const own: ChatGroup<Row> = { kind: 'own', key: BOT_DEFAULT_WORKSPACE_ID, title: ownFolderTitle(), chats: [] };
  const bound: ChatGroup<Row>[] = bot.workspaces.map((folder) => ({ kind: 'folder', key: folder.id, title: folderTitle(folder), path: folder.path, workspaceId: folder.id, chats: [] }));
  const removed: ChatGroup<Row> = { kind: 'removed', key: 'removed', title: tr('bots_folders_removed'), chats: [] };
  for (const chat of chats) {
    if (chat.removedPath !== undefined) removed.chats.push(chat);
    else (bound.find((group) => group.workspaceId === chat.workspaceId) ?? own).chats.push(chat);
  }
  const groups = [own, ...bound, ...(removed.chats.length > 0 ? [removed] : [])];
  return { groups, headed: groups.length > 1 };
}
