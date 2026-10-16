import assert from 'node:assert/strict';
import test from 'node:test';
import { restoreSlashDraftAfterTerminalError } from '../src/renderer/bridge/slashDraftRecovery';
import { SlashCommandTerminalRegistry } from '../src/renderer/bridge/slashCommandTerminals';
import {
  acknowledgeSideSlashPrompt,
  isSideSlashTerminalError,
  restoreSideSlashPromptAfterError,
  type SideSlashSubmissionSnapshot,
} from '../src/renderer/bridge/sideSlashDraftRecovery';
import type { SideChatState } from '../src/renderer/bridge/sideChatState';
import type { ImageAttachment } from '../src/renderer/bridge/imageInput';

const image = (id: string): ImageAttachment => ({
  id,
  name: `${id}.png`,
  media_type: 'image/png',
  base64: 'AQID',
  previewUrl: 'data:image/png;base64,AQID',
});

test('desktop restores the exact admitted image draft only for its matching asynchronous slash error', () => {
  const submitted = {
    sessionId: 'session-a',
    generation: 5,
    html: '<span>/inspect</span>',
    text: '/inspect',
    files: ['src/main.ts'],
    imageIds: ['image-a'],
    images: [image('image-a')],
  };
  const owner = { sessionId: 'session-a', draftGeneration: 12 };
  const current = {
    activeSessionId: 'session-a',
    draftGeneration: 12,
    savedDraftExists: false,
    text: '',
    files: [],
    images: [],
  };
  const registry = new SlashCommandTerminalRegistry();
  let restored: ReturnType<typeof restoreSlashDraftAfterTerminalError> = null;
  registry.register('session-a', 81, (terminal) => {
    restored = restoreSlashDraftAfterTerminalError(submitted, owner, terminal, current);
  });

  assert.equal(registry.observe('session-a', { type: 'slash_command_result', turn_id: 80, display: 'wrong', is_error: true }), false);
  assert.equal(restored, null);
  assert.equal(registry.observe('session-b', { type: 'slash_command_result', turn_id: 81, display: 'wrong session', is_error: true }), false);
  assert.equal(registry.observe('session-a', { type: 'error', kind: { type: 'transport' }, message: 'unrelated turn error' }), false);
  assert.equal(restored, null);
  assert.equal(registry.observe('session-a', { type: 'slash_command_result', turn_id: 81, display: 'failed', is_error: true }), true);
  assert.deepEqual(restored, {
    kind: 'visible', text: '/inspect', html: '<span>/inspect</span>', files: ['src/main.ts'], images: [image('image-a')],
  });
  assert.equal(registry.observe('session-a', { type: 'slash_command_result', turn_id: 81, display: 'duplicate', is_error: true }), false);
  assert.deepEqual(restoreSlashDraftAfterTerminalError(submitted, owner, { turnId: 84, display: 'failed', isError: true }, {
    activeSessionId: 'session-b', draftGeneration: 12, savedDraftExists: false, text: '', files: [], images: [],
  })?.kind, 'saved');
  assert.equal(restoreSlashDraftAfterTerminalError(submitted, owner, { turnId: 84, display: 'failed', isError: true }, {
    activeSessionId: 'session-b', draftGeneration: 12, savedDraftExists: true, text: '', files: [], images: [],
  }), null);
});

test('desktop does not restore after a successful turn start or a later draft edit', () => {
  const submitted = {
    sessionId: 'session-a', generation: 5, text: '/inspect', files: [], imageIds: ['image-a'], images: [image('image-a')],
  };
  const owner = { sessionId: 'session-a', draftGeneration: 12 };
  const edited = { activeSessionId: 'session-a', draftGeneration: 13, savedDraftExists: false, text: 'new draft', files: [], images: [] };
  const registry = new SlashCommandTerminalRegistry();
  let restored: ReturnType<typeof restoreSlashDraftAfterTerminalError> = null;
  registry.register('session-a', 82, (terminal) => {
    restored = restoreSlashDraftAfterTerminalError(submitted, owner, terminal, edited);
  });
  assert.equal(registry.observe('session-a', { type: 'turn_started', turn_id: 82 }), true);
  assert.equal(restored, null);
  assert.equal(registry.observe('session-a', { type: 'slash_command_result', turn_id: 82, display: 'late', is_error: true }), false);

  const terminalError = { turnId: 83, display: 'failed', isError: true };
  assert.equal(restoreSlashDraftAfterTerminalError(submitted, owner, terminalError, edited), null);
});

test('Side Chat keeps slash images through admission and restores only on a matching asynchronous terminal error', () => {
  const attachment = image('side-image');
  const side = { id: 'side-a', hostId: 'host-a', draft: '/inspect', images: [attachment], draftRevision: 4 } as SideChatState;
  const submission: SideSlashSubmissionSnapshot = {
    hostId: 'host-a', sideId: 'side-a', turnId: 91, text: '/inspect', images: [attachment],
    draftRevision: 4, acknowledged: false, terminalError: false,
  };
  const admitted = acknowledgeSideSlashPrompt(side, submission);
  submission.acknowledged = true;
  assert.equal(admitted.draft, '');
  assert.deepEqual(admitted.images, []);

  const wrongTurn = { type: 'slash_command_result', turn_id: 90, display: 'wrong', is_error: true } as const;
  const failed = { type: 'slash_command_result', turn_id: 91, display: 'failed', is_error: true } as const;
  assert.equal(isSideSlashTerminalError(wrongTurn, submission), false);
  assert.equal(isSideSlashTerminalError(failed, submission), true);
  const terminal = isSideSlashTerminalError(failed, submission);
  assert.equal(terminal, true);
  const restored = terminal ? restoreSideSlashPromptAfterError(admitted, submission) : admitted;
  assert.equal(restored.draft, '/inspect');
  assert.deepEqual(restored.images.map(({ id }) => id), ['side-image']);
});

test('Side Chat preserves an early terminal error and never overwrites a newer edit', () => {
  const attachment = image('side-image');
  const side = { id: 'side-a', hostId: 'host-a', draft: '/inspect', images: [attachment], draftRevision: 4 } as SideChatState;
  const submission: SideSlashSubmissionSnapshot = {
    hostId: 'host-a', sideId: 'side-a', turnId: 92, text: '/inspect', images: [attachment],
    draftRevision: 4, acknowledged: false, terminalError: true,
  };
  assert.equal(acknowledgeSideSlashPrompt(side, submission), side);
  submission.acknowledged = true;
  const acked = { ...side, draft: '', images: [] };
  const edited = { ...acked, draft: 'new text', draftRevision: 5 };
  assert.equal(restoreSideSlashPromptAfterError(edited, submission), edited);
});
