import assert from 'node:assert/strict';
import test from 'node:test';

import { BOT_CONFLICT_MESSAGE, type BotMemoryEntryUi, type BotSkillUi, type BotUi, type BotsApi, type BotsAvailability } from '../src/shared/bots';
import { BotEditorController, botEditorDirty } from '../src/renderer/bridge/botEditor';
import { BOT_MEMORY_MAX_BYTES, BotMemoryController, botMemoryProblem } from '../src/renderer/bridge/botMemory';
import { BotSkillsController, importCandidates } from '../src/renderer/bridge/botSkills';
import { BotsController } from '../src/renderer/bridge/botsStore';

function bot(overrides: Partial<BotUi> = {}): BotUi {
  return { botId: 'b1', name: 'Scribe', description: 'Writes things down', createdAt: 1_000, updatedAt: 1_000, revision: 1, workspaces: [], ...overrides };
}

function ipc(message: string): Error {
  return new Error(`Error invoking remote method 'lingxi:bots': Error: ${message}`);
}

/** Resolve on the next turn of the event loop, after every pending promise has settled. */
const settle = () => new Promise<void>((resolve) => setImmediate(resolve));

// ---- The list and whether Bots exist here at all ----

function listApi(availability: BotsAvailability | Error, list: () => Promise<BotUi[]>) {
  const calls = { availability: 0, list: 0 };
  const api: Pick<BotsApi, 'availability' | 'list'> = {
    async availability() { calls.availability++; if (availability instanceof Error) throw availability; return availability; },
    async list() { calls.list++; return list(); },
  };
  return { api, calls };
}

test('a window whose host has no Bots API has no Bots to show', async () => {
  const controller = new BotsController(undefined);
  await controller.start();
  assert.deepEqual(controller.getSnapshot(), { availability: { available: false }, bots: [], loading: false, error: undefined });
});

test('where Bots cannot be used the list is never asked for, whether the host says so or cannot say', async () => {
  const refused = listApi({ available: false, reason: 'Bots are available on macOS only.' }, async () => [bot()]);
  const controller = new BotsController(refused.api);
  await controller.start();
  assert.equal(controller.getSnapshot().availability?.available, false);
  assert.equal(controller.getSnapshot().availability?.reason, 'Bots are available on macOS only.');
  assert.equal(refused.calls.list, 0);
  await controller.reload();
  assert.equal(refused.calls.list, 0);

  const silent = listApi(ipc('no handler registered'), async () => [bot()]);
  const unsure = new BotsController(silent.api);
  await unsure.start();
  assert.equal(unsure.getSnapshot().availability?.available, false);
  assert.equal(unsure.getSnapshot().availability?.reason, 'no handler registered');
  assert.equal(silent.calls.list, 0);
});

test('where Bots can be used the list is read, and Bots can be added, replaced and removed', async () => {
  const { api, calls } = listApi({ available: true }, async () => [bot({ botId: 'a', name: 'Alpha' })]);
  const controller = new BotsController(api);
  const seen: boolean[] = [];
  controller.subscribe(() => seen.push(controller.getSnapshot().loading));
  await controller.start();
  assert.equal(calls.list, 1);
  assert.deepEqual(controller.getSnapshot().bots.map((entry) => entry.name), ['Alpha']);
  assert.equal(controller.getSnapshot().loading, false);
  assert.ok(seen.includes(true), 'the window is told the list is loading');

  controller.put(bot({ botId: 'b', name: 'Beta' }));
  controller.put(bot({ botId: 'a', name: 'Alpha 2', revision: 2 }));
  assert.deepEqual(controller.getSnapshot().bots.map((entry) => entry.name), ['Alpha 2', 'Beta']);
  controller.remove('a');
  assert.deepEqual(controller.getSnapshot().bots.map((entry) => entry.botId), ['b']);
});

test('a list that cannot be read is an error to show, and a later answer replaces it', async () => {
  let fail = true;
  const { api } = listApi({ available: true }, async () => { if (fail) throw ipc('The Bots folder cannot be read.'); return [bot()]; });
  const controller = new BotsController(api);
  await controller.start();
  assert.equal(controller.getSnapshot().error, 'The Bots folder cannot be read.');
  assert.equal(controller.getSnapshot().loading, false);
  fail = false;
  await controller.reload();
  assert.equal(controller.getSnapshot().error, undefined);
  assert.equal(controller.getSnapshot().bots.length, 1);
});

test('an older answer never replaces a newer one', async () => {
  const resolvers: Array<(bots: BotUi[]) => void> = [];
  const { api } = listApi({ available: true }, () => new Promise<BotUi[]>((resolve) => { resolvers.push(resolve); }));
  const controller = new BotsController(api);
  void controller.start();
  await settle();
  const second = controller.reload();
  await settle();
  assert.equal(resolvers.length, 2);
  resolvers[1]!([bot({ name: 'Newer' })]);
  await second;
  resolvers[0]!([bot({ name: 'Older' })]);
  await settle();
  assert.deepEqual(controller.getSnapshot().bots.map((entry) => entry.name), ['Newer']);
});

// ---- One Bot open for editing ----

function editorApi(initial: BotUi, options: { persona?: string; update?: (patch: Record<string, unknown>, revision: number) => BotUi | Error; personaSet?: Error } = {}) {
  const state = { bot: initial, persona: options.persona ?? 'old persona', updates: [] as Array<[number, Record<string, unknown>]>, personaWrites: [] as string[] };
  const api = {
    async get() { return state.bot; },
    async update(_botId: string, revision: number, patch: Record<string, unknown>) {
      state.updates.push([revision, patch]);
      const result = options.update?.(patch, revision) ?? { ...state.bot, ...patch, revision: state.bot.revision + 1 };
      if (result instanceof Error) throw result;
      state.bot = result as BotUi;
      return state.bot;
    },
    persona: {
      async get() { return state.persona; },
      async set(_botId: string, text: string) {
        state.personaWrites.push(text);
        if (options.personaSet) throw options.personaSet;
        state.persona = text;
      },
    },
  };
  return { api: api as unknown as Pick<BotsApi, 'get' | 'update' | 'persona'>, state };
}

test('opening a Bot reads it and its persona into a draft that is not yet changed', async () => {
  const { api } = editorApi(bot());
  const records: BotUi[] = [];
  const editor = new BotEditorController(api, 'b1', (record) => records.push(record));
  assert.equal(editor.getSnapshot().phase, 'loading');
  await editor.load();
  const state = editor.getSnapshot();
  assert.equal(state.phase, 'ready');
  assert.equal(state.draft.name, 'Scribe');
  assert.equal(state.draft.persona, 'old persona');
  assert.equal(botEditorDirty(state), false);
  assert.deepEqual(records.map((record) => record.revision), [1]);
});

test('a Bot that cannot be read says so and can be tried again', async () => {
  let fail = true;
  const { api } = editorApi(bot());
  const flaky = { ...api, get: async () => { if (fail) throw ipc('No such Bot.'); return bot(); } };
  const editor = new BotEditorController(flaky as unknown as typeof api, 'b1', () => undefined);
  await editor.load();
  assert.equal(editor.getSnapshot().phase, 'failed');
  assert.equal(editor.getSnapshot().loadError, 'No such Bot.');
  fail = false;
  await editor.load();
  assert.equal(editor.getSnapshot().phase, 'ready');
});

test('saving sends the change at the revision that was read, and the draft becomes the saved Bot', async () => {
  const { api, state } = editorApi(bot());
  const records: BotUi[] = [];
  const editor = new BotEditorController(api, 'b1', (record) => records.push(record));
  await editor.load();
  editor.setDraft({ name: 'Renamed', persona: 'new persona' });
  assert.equal(botEditorDirty(editor.getSnapshot()), true);
  await editor.save();
  assert.deepEqual(state.updates, [[1, { name: 'Renamed' }]]);
  assert.deepEqual(state.personaWrites, ['new persona']);
  const after = editor.getSnapshot();
  assert.equal(after.save.kind, 'saved');
  assert.equal(after.bot?.revision, 2);
  assert.equal(after.persona, 'new persona');
  assert.equal(botEditorDirty(after), false);
  assert.deepEqual(records.map((record) => record.revision), [1, 2]);
});

test('a draft with problems, or nothing to save, sends nothing', async () => {
  const { api, state } = editorApi(bot());
  const editor = new BotEditorController(api, 'b1', () => undefined);
  await editor.load();
  await editor.save();
  editor.setDraft({ name: '' });
  await editor.save();
  assert.deepEqual(state.updates, []);
  assert.deepEqual(state.personaWrites, []);
  assert.equal(editor.getSnapshot().save.kind, 'idle');
});

test('a conflict is shown as "changed elsewhere", keeps the edits until the owner reloads, and reloading replaces them', async () => {
  let elsewhere = false;
  const { api, state } = editorApi(bot(), { update: () => elsewhere ? new Error(BOT_CONFLICT_MESSAGE) : undefined as unknown as BotUi });
  const editor = new BotEditorController(api, 'b1', () => undefined);
  await editor.load();
  // Someone else (another window, or the Bot itself) changes the Bot after it was read here.
  state.bot = bot({ revision: 2, name: 'Changed elsewhere' });
  elsewhere = true;
  editor.setDraft({ name: 'My edit', persona: 'my persona' });
  await editor.save();
  assert.equal(editor.getSnapshot().save.kind, 'conflict');
  assert.equal(editor.getSnapshot().draft.name, 'My edit', 'the owner keeps their text on screen');
  assert.deepEqual(state.personaWrites, [], 'nothing was written');

  await editor.save();
  assert.equal(editor.getSnapshot().save.kind, 'conflict');

  await editor.load();
  const reloaded = editor.getSnapshot();
  assert.equal(reloaded.save.kind, 'idle');
  assert.equal(reloaded.bot?.revision, 2);
  assert.equal(reloaded.draft.name, 'Changed elsewhere');
  assert.equal(reloaded.draft.persona, 'old persona');
});

test('when the record was written but the persona was not, trying again writes only the persona', async () => {
  const { api, state } = editorApi(bot(), { personaSet: ipc('The persona could not be written.') });
  const editor = new BotEditorController(api, 'b1', () => undefined);
  await editor.load();
  editor.setDraft({ name: 'Renamed', persona: 'new persona' });
  await editor.save();
  const failed = editor.getSnapshot();
  assert.deepEqual(failed.save, { kind: 'failed', message: 'The persona could not be written.' });
  assert.equal(failed.bot?.name, 'Renamed');
  assert.equal(failed.bot?.revision, 2);
  assert.equal(botEditorDirty(failed), true, 'the persona is still unsaved');

  state.updates.length = 0;
  await editor.save();
  assert.deepEqual(state.updates, [], 'the record is not sent again, so it cannot conflict with itself');
  assert.deepEqual(state.personaWrites, ['new persona', 'new persona']);
});

test('edits are ignored while a save is in flight', async () => {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const { api } = editorApi(bot());
  const slow = { ...api, update: async (...args: Parameters<typeof api.update>) => { await gate; return api.update(...args); } };
  const editor = new BotEditorController(slow as typeof api, 'b1', () => undefined);
  await editor.load();
  editor.setDraft({ name: 'Renamed' });
  const saving = editor.save();
  await settle();
  assert.equal(editor.getSnapshot().save.kind, 'saving');
  editor.setDraft({ name: 'Too late' });
  assert.equal(editor.getSnapshot().draft.name, 'Renamed');
  release();
  await saving;
  assert.equal(editor.getSnapshot().save.kind, 'saved');
});

// ---- Memory ----

function memoryApi(entries: BotMemoryEntryUi[], files: Record<string, string>, options: { deleteError?: Error; writeError?: Error } = {}) {
  const calls = { writes: [] as Array<[string, string]>, deletes: [] as string[] };
  const api: BotsApi['memory'] = {
    async list() { return entries.map((entry) => ({ ...entry })); },
    async read(_botId, name) { if (!(name in files)) throw ipc('No such memory.'); return files[name]!; },
    async write(_botId, name, content) {
      calls.writes.push([name, content]);
      if (options.writeError) throw options.writeError;
      files[name] = content;
    },
    async delete(_botId, name) {
      calls.deletes.push(name);
      if (options.deleteError) throw options.deleteError;
      delete files[name];
      const index = entries.findIndex((entry) => entry.name === name);
      if (index >= 0) entries.splice(index, 1);
    },
  };
  return { api, calls };
}

const MEMORY: BotMemoryEntryUi[] = [
  { name: 'user-role.md', description: 'Who the owner is', type: 'user', size: 40, modifiedAt: 2_000 },
  { name: 'style.md', size: 20, modifiedAt: 1_000 },
];

test('memory lists name, description, type and time, and an entry can be looked at', async () => {
  const { api } = memoryApi(structuredClone(MEMORY), { 'user-role.md': 'The owner writes Rust.', 'style.md': 'Short answers.' });
  const controller = new BotMemoryController(api, 'b1');
  assert.equal(controller.getSnapshot().entries, undefined);
  await controller.load();
  assert.deepEqual(controller.getSnapshot().entries?.map((entry) => [entry.name, entry.description, entry.type]), [['user-role.md', 'Who the owner is', 'user'], ['style.md', undefined, undefined]]);
  await controller.open('user-role.md');
  assert.deepEqual(controller.getSnapshot().open, { name: 'user-role.md', content: 'The owner writes Rust.', draft: 'The owner writes Rust.', editing: false, busy: false, error: undefined });
  controller.close();
  assert.equal(controller.getSnapshot().open, undefined);
});

test('an entry that cannot be read shows why and cannot be edited', async () => {
  const { api } = memoryApi(structuredClone(MEMORY), {});
  const controller = new BotMemoryController(api, 'b1');
  await controller.open('gone.md');
  assert.equal(controller.getSnapshot().open?.error, 'No such memory.');
  controller.edit();
  assert.equal(controller.getSnapshot().open?.editing, false);
});

test('editing writes only a changed text, then the list is read again', async () => {
  const entries = structuredClone(MEMORY);
  const { api, calls } = memoryApi(entries, { 'style.md': 'Short answers.', 'user-role.md': 'x' });
  const controller = new BotMemoryController(api, 'b1');
  await controller.load();
  await controller.open('style.md');
  controller.edit();
  await controller.save();
  assert.deepEqual(calls.writes, [], 'an entry that was not changed is not written');
  assert.equal(controller.getSnapshot().open?.editing, false);

  controller.edit();
  controller.setDraft('Short answers, no emoji.');
  await controller.save();
  assert.deepEqual(calls.writes, [['style.md', 'Short answers, no emoji.']]);
  assert.equal(controller.getSnapshot().open?.content, 'Short answers, no emoji.');
  assert.equal(controller.getSnapshot().open?.editing, false);
});

test('a text that cannot be saved is refused before it is sent, and a failed write keeps the edit', async () => {
  assert.equal(botMemoryProblem('fine'), undefined);
  assert.equal(botMemoryProblem('bad \0 text'), 'invalid');
  assert.equal(botMemoryProblem('x'.repeat(BOT_MEMORY_MAX_BYTES + 1)), 'too_large');

  const { api, calls } = memoryApi(structuredClone(MEMORY), { 'style.md': 'a' }, { writeError: ipc('The disk is full.') });
  const controller = new BotMemoryController(api, 'b1');
  await controller.open('style.md');
  controller.edit();
  controller.setDraft('bad \0 text');
  await controller.save();
  assert.deepEqual(calls.writes, []);

  controller.setDraft('b');
  await controller.save();
  assert.deepEqual(calls.writes, [['style.md', 'b']]);
  const open = controller.getSnapshot().open;
  assert.equal(open?.error, 'The disk is full.');
  assert.equal(open?.editing, true);
  assert.equal(open?.draft, 'b');
  assert.equal(open?.content, 'a');
});

test('deleting asks first: nothing is deleted until the owner confirms, and cancelling leaves the entry', async () => {
  const entries = structuredClone(MEMORY);
  const { api, calls } = memoryApi(entries, { 'style.md': 'a', 'user-role.md': 'b' });
  const controller = new BotMemoryController(api, 'b1');
  await controller.load();
  await controller.open('style.md');

  controller.askDelete('style.md');
  assert.deepEqual(controller.getSnapshot().deleting, { name: 'style.md', busy: false, error: undefined });
  assert.deepEqual(calls.deletes, []);
  controller.cancelDelete();
  assert.equal(controller.getSnapshot().deleting, undefined);
  assert.deepEqual(calls.deletes, []);

  controller.askDelete('style.md');
  await controller.confirmDelete();
  assert.deepEqual(calls.deletes, ['style.md']);
  assert.equal(controller.getSnapshot().deleting, undefined);
  assert.equal(controller.getSnapshot().open, undefined, 'the entry that was open is closed with it');
  assert.deepEqual(controller.getSnapshot().entries?.map((entry) => entry.name), ['user-role.md']);
});

test('a deletion that fails stays on screen with the reason, and is not repeated by a second confirm while it runs', async () => {
  const { api, calls } = memoryApi(structuredClone(MEMORY), { 'style.md': 'a' }, { deleteError: ipc('The file is in use.') });
  const controller = new BotMemoryController(api, 'b1');
  await controller.load();
  controller.askDelete('style.md');
  const first = controller.confirmDelete();
  const second = controller.confirmDelete();
  await Promise.all([first, second]);
  assert.deepEqual(calls.deletes, ['style.md']);
  assert.deepEqual(controller.getSnapshot().deleting, { name: 'style.md', busy: false, error: 'The file is in use.' });
  assert.deepEqual(controller.getSnapshot().entries?.map((entry) => entry.name), ['user-role.md', 'style.md']);
});

// ---- Skills ----

const OWNER_SKILLS: BotSkillUi[] = [{ name: 'commit', description: 'Write a commit message' }, { name: 'review' }, { name: 'translate', description: 'Translate text' }];

function skillsApi(present: BotSkillUi[], importable: BotSkillUi[], options: { importError?: Error; deleteError?: Error } = {}) {
  const calls = { imports: [] as string[], deletes: [] as string[] };
  const api: BotsApi['skills'] = {
    async list() { return present.map((skill) => ({ ...skill })); },
    async listImportable() { return importable.map((skill) => ({ ...skill })); },
    async importFromUser(_botId, name) {
      calls.imports.push(name);
      if (options.importError) throw options.importError;
      const skill = importable.find((entry) => entry.name === name)!;
      present.push({ ...skill });
      return skill;
    },
    async delete(_botId, name) {
      calls.deletes.push(name);
      if (options.deleteError) throw options.deleteError;
      present.splice(present.findIndex((skill) => skill.name === name), 1);
    },
  };
  return { api, calls };
}

test('only the skills a Bot does not have yet are offered for import', () => {
  assert.deepEqual(importCandidates(OWNER_SKILLS, [{ name: 'review' }]).map((skill) => skill.name), ['commit', 'translate']);
  assert.deepEqual(importCandidates(OWNER_SKILLS, OWNER_SKILLS), []);
  assert.deepEqual(importCandidates([], [{ name: 'review' }]), []);
});

test('importing lists the owner\'s skills the Bot lacks, copies the chosen one once, and shows it among the Bot\'s own', async () => {
  const { api, calls } = skillsApi([{ name: 'review' }], OWNER_SKILLS);
  const controller = new BotSkillsController(api, 'b1');
  await controller.load();
  assert.deepEqual(controller.getSnapshot().skills?.map((skill) => skill.name), ['review']);

  await controller.openImport();
  assert.deepEqual(controller.getSnapshot().importing?.candidates?.map((skill) => skill.name), ['commit', 'translate']);
  await controller.importSkill('commit');
  assert.deepEqual(calls.imports, ['commit']);
  assert.deepEqual(controller.getSnapshot().importing?.candidates?.map((skill) => skill.name), ['translate']);
  assert.deepEqual(controller.getSnapshot().skills?.map((skill) => skill.name), ['review', 'commit']);
  controller.closeImport();
  assert.equal(controller.getSnapshot().importing, undefined);
});

test('an import that fails keeps the choice open and says why', async () => {
  const { api } = skillsApi([], OWNER_SKILLS, { importError: ipc('This skill is too large.') });
  const controller = new BotSkillsController(api, 'b1');
  await controller.openImport();
  await controller.importSkill('commit');
  const importing = controller.getSnapshot().importing;
  assert.equal(importing?.error, 'This skill is too large.');
  assert.equal(importing?.busy, undefined);
  assert.deepEqual(importing?.candidates?.map((skill) => skill.name), ['commit', 'review', 'translate']);
});

test('skills are deleted only after the owner confirms', async () => {
  const { api, calls } = skillsApi([{ name: 'review' }, { name: 'commit' }], []);
  const controller = new BotSkillsController(api, 'b1');
  await controller.load();
  controller.askDelete('review');
  controller.cancelDelete();
  assert.deepEqual(calls.deletes, []);
  controller.askDelete('review');
  await controller.confirmDelete();
  assert.deepEqual(calls.deletes, ['review']);
  assert.deepEqual(controller.getSnapshot().skills?.map((skill) => skill.name), ['commit']);
  assert.equal(controller.getSnapshot().deleting, undefined);
});
