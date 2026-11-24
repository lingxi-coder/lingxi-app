import assert from 'node:assert/strict';
import test from 'node:test';

import type { BotSessionRowUi, BotWorkspaceUi } from '../src/shared/bots';
import { chatGroups, folderBaseName, folderTitle, ownFolderTitle, workspaceIdAtPath } from '../src/renderer/bridge/botFolders';
import { pinEnglish } from './bot-ui-helpers';

pinEnglish();

function folder(id: string, path: string, label?: string): BotWorkspaceUi {
  return { id, path, mode: 'read', addedAt: 1, ...(label !== undefined ? { label } : {}) };
}

type Row = Pick<BotSessionRowUi, 'workspaceId' | 'removedPath'> & { uuid: string };
const row = (uuid: string, where: { workspaceId?: string; removedPath?: string }): Row => ({ uuid, ...where });

test('a folder is called what the owner named it, else by the last part of its path', () => {
  assert.equal(folderBaseName('/Users/me/Documents/Specs'), 'Specs');
  assert.equal(folderBaseName('/Users/me/Documents/Specs/'), 'Specs');
  assert.equal(folderBaseName('C:\\Work\\Specs\\'), 'Specs');
  assert.equal(folderBaseName('/'), '/');
  assert.equal(folderTitle({ path: '/Users/me/Specs' }), 'Specs');
  assert.equal(folderTitle({ path: '/Users/me/Specs', label: ' Product specs ' }), 'Product specs');
  assert.equal(folderTitle({ path: '/Users/me/Specs', label: '  ' }), 'Specs');
  assert.equal(ownFolderTitle(), 'Own workspace');
});

test('a chat works in the folder of the Bot that its project path is, else in the Bot\'s own', () => {
  const bot = { workspaces: [folder('wA', '/work/a'), folder('wB', '/work/b')] };
  assert.equal(workspaceIdAtPath(bot, '/work/b'), 'wB');
  assert.equal(workspaceIdAtPath(bot, '/data/bots/x/workspace'), 'default');
  assert.equal(workspaceIdAtPath(bot, 'bot:x'), 'default', 'the placeholder of a chat that is still starting');
  assert.equal(workspaceIdAtPath(bot, undefined), 'default');
});

test('a Bot with no folders but its own has one list of chats, without headings', () => {
  const chats = [row('s1', { workspaceId: 'default' }), row('s2', {}), row('s3', { workspaceId: 'default' })];
  const { groups, headed } = chatGroups({ workspaces: [] }, chats);
  assert.equal(headed, false);
  assert.equal(groups.length, 1);
  assert.equal(groups[0]!.kind, 'own');
  assert.deepEqual(groups[0]!.chats.map((chat) => chat.uuid), ['s1', 's2', 's3'], 'in the order they came');
});

test('chats are grouped by the folder they worked in: the Bot\'s own first, the folders in their order, the removed ones last', () => {
  const bot = { workspaces: [folder('wA', '/work/a', 'Notes'), folder('wB', '/work/b')] };
  const chats = [
    row('s1', { workspaceId: 'wB' }), row('s2', { workspaceId: 'default' }), row('s3', { removedPath: '/work/gone' }),
    row('s4', { workspaceId: 'wA' }), row('s5', { workspaceId: 'wB' }), row('s6', { workspaceId: 'wUnknown' }),
  ];
  const { groups, headed } = chatGroups(bot, chats);
  assert.equal(headed, true);
  assert.deepEqual(groups.map((group) => [group.kind, group.key, group.title, group.workspaceId, group.chats.map((chat) => chat.uuid)]), [
    ['own', 'default', 'Own workspace', undefined, ['s2', 's6']],
    ['folder', 'wA', 'Notes', 'wA', ['s4']],
    ['folder', 'wB', 'b', 'wB', ['s1', 's5']],
    ['removed', 'removed', 'Removed folders', undefined, ['s3']],
  ]);
  assert.deepEqual(groups.filter((group) => group.kind === 'folder').map((group) => group.path), ['/work/a', '/work/b']);
});

test('a folder with no chats is still a heading, so a chat can be started in it, and removed folders are listed only when they have chats', () => {
  const bot = { workspaces: [folder('wA', '/work/a')] };
  const { groups, headed } = chatGroups(bot, [row('s1', { workspaceId: 'default' })]);
  assert.equal(headed, true);
  assert.deepEqual(groups.map((group) => [group.key, group.chats.length]), [['default', 1], ['wA', 0]]);
  // Only chats of removed folders make a Bot's list headed when it has no folders left.
  const later = chatGroups({ workspaces: [] }, [row('s1', { workspaceId: 'default' }), row('s2', { removedPath: '/work/a' })]);
  assert.equal(later.headed, true);
  assert.deepEqual(later.groups.map((group) => group.key), ['default', 'removed']);
});
