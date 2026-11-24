import assert from 'node:assert/strict';
import test from 'node:test';

import { BOT_ALLOWED_DOMAINS_MAX, BOT_CONFLICT_MESSAGE, BOT_SANDBOX_UNAVAILABLE_MESSAGE, allowedDomain, type BotCreateRequest, type BotUi, type BotUpdateRequest } from '../src/shared/bots';
import {
  botChanges,
  botCreateRequest,
  botDraftProblems,
  createBot,
  draftFromBot,
  draftWithNewFolders,
  emptyBotDraft,
  hasBotDraftProblems,
  ipcErrorMessage,
  isBotConflict,
  isBotSandboxUnavailable,
  saveBot,
  sortBots,
  withAllowedHost,
  withFolder,
  type BotDraft,
} from '../src/renderer/bridge/botForm';

function bot(overrides: Partial<BotUi> = {}): BotUi {
  return { botId: 'b1', name: 'Scribe', description: 'Writes things down', createdAt: 1_000, updatedAt: 1_000, revision: 3, workspaces: [], sandbox: { enabled: true, allowedDomains: [] }, ...overrides };
}

/** The wrapper Electron puts around an error thrown by an IPC handler. */
function overIpc(channel: string, message: string): Error {
  return new Error(`Error invoking remote method '${channel}': Error: ${message}`);
}

interface Calls { update: Array<[string, number, BotUpdateRequest]>; persona: Array<[string, string]>; create: BotCreateRequest[] }

function fakeApi(options: { update?: (patch: BotUpdateRequest) => BotUi | Error; persona?: Error; create?: BotUi | Error } = {}) {
  const calls: Calls = { update: [], persona: [], create: [] };
  const api = {
    async create(request: BotCreateRequest) {
      calls.create.push(request);
      if (options.create instanceof Error) throw options.create;
      return options.create ?? bot({ name: request.name });
    },
    async update(botId: string, revision: number, patch: BotUpdateRequest) {
      calls.update.push([botId, revision, patch]);
      const result = options.update?.(patch) ?? bot({ revision: revision + 1, ...(patch.name !== undefined ? { name: patch.name } : {}) });
      if (result instanceof Error) throw result;
      return result;
    },
    persona: {
      async set(botId: string, text: string) {
        calls.persona.push([botId, text]);
        if (options.persona) throw options.persona;
      },
    },
  };
  return { api, calls };
}

test('an empty draft is missing a name and nothing else', () => {
  assert.deepEqual(botDraftProblems(emptyBotDraft()), { name: 'name_required' });
  assert.deepEqual(botDraftProblems({ ...emptyBotDraft(), name: '   ' }), { name: 'name_required' });
  assert.equal(hasBotDraftProblems(botDraftProblems({ ...emptyBotDraft(), name: 'Scribe' })), false);
});

test('each limit the main process enforces is reported before anything is sent', () => {
  const draft = (change: Partial<BotDraft>): BotDraft => ({ ...emptyBotDraft(), name: 'Scribe', ...change });
  assert.deepEqual(botDraftProblems(draft({ name: 'x'.repeat(81) })), { name: 'name_too_long' });
  assert.deepEqual(botDraftProblems(draft({ name: 'x'.repeat(80) })), {});
  assert.deepEqual(botDraftProblems(draft({ name: 'two\nlines' })), { name: 'name_invalid' });
  assert.deepEqual(botDraftProblems(draft({ description: 'x'.repeat(501) })), { description: 'description_too_long' });
  // The persona limit is in bytes: 22 000 three-byte characters are 66 000 bytes.
  assert.deepEqual(botDraftProblems(draft({ persona: '界'.repeat(22_000) })), { persona: 'persona_too_long' });
  assert.deepEqual(botDraftProblems(draft({ persona: '界'.repeat(21_000) })), {});
  const many = Array.from({ length: 17 }, (_, index) => ({ path: `/work/${index}`, mode: 'read' as const }));
  assert.deepEqual(botDraftProblems(draft({ workspaces: many })), { folders: 'folders_too_many' });
  assert.deepEqual(botDraftProblems(draft({ workspaces: [{ path: '/work/a', mode: 'read' }, { path: '/work/a', mode: 'write' }] })), { folders: 'folders_duplicate' });
});

test('a new Bot sends the displayed approval mode and the trimmed fields the owner filled in', () => {
  assert.deepEqual(botCreateRequest({ ...emptyBotDraft(), name: '  Scribe  ' }), { name: 'Scribe', permissionMode: 'default' });
  assert.deepEqual(botCreateRequest({
    name: 'Scribe', description: ' notes ', persona: '# Scribe\n', model: 'builtin/small', permissionMode: 'plan',
    workspaces: [{ path: '/work/a', mode: 'write' }], acceptFrom: [], remote: false, sandbox: { enabled: true, allowedDomains: [] },
  }), { name: 'Scribe', description: 'notes', persona: '# Scribe\n', model: 'builtin/small', permissionMode: 'plan', workspaces: [{ path: '/work/a', mode: 'write' }] });
});

test('a new Bot is made out of reach of remote devices: its draft has the switch off and the request never carries it', () => {
  assert.equal(emptyBotDraft().remote, false);
  const request = botCreateRequest({ ...emptyBotDraft(), name: 'Scribe', remote: true });
  assert.deepEqual(request, { name: 'Scribe', permissionMode: 'default' });
  assert.ok(!('remote' in request));
});

test('creating a Bot sends nothing while the form has problems, and says why a refusal happened otherwise', async () => {
  const refused = fakeApi();
  assert.deepEqual(await createBot(refused.api, emptyBotDraft()), { ok: false, conflict: false, message: '' });
  assert.equal(refused.calls.create.length, 0);

  const created = fakeApi();
  const result = await createBot(created.api, { ...emptyBotDraft(), name: 'Scribe', persona: 'Be brief.' });
  assert.equal(result.ok, true);
  assert.deepEqual(created.calls.create, [{ name: 'Scribe', persona: 'Be brief.', permissionMode: 'default' }]);

  const failing = fakeApi({ create: overIpc('lingxi:bots:create', 'A Bot named “Scribe” already exists.') });
  assert.deepEqual(await createBot(failing.api, { ...emptyBotDraft(), name: 'Scribe' }), { ok: false, conflict: false, message: 'A Bot named “Scribe” already exists.' });
});

test('errors from the main process are read as their message, and a conflict by that message', () => {
  assert.equal(ipcErrorMessage(overIpc('lingxi:bots:update', 'No such Bot.')), 'No such Bot.');
  assert.equal(ipcErrorMessage(new Error('plain')), 'plain');
  assert.equal(ipcErrorMessage('text'), 'text');
  assert.equal(ipcErrorMessage(undefined), '');
  assert.equal(isBotConflict(new Error(BOT_CONFLICT_MESSAGE)), true);
  assert.equal(isBotConflict(overIpc('lingxi:bots:update', BOT_CONFLICT_MESSAGE)), true);
  assert.equal(isBotConflict(overIpc('lingxi:bots:update', 'No such Bot.')), false);
});

test('explicit approval stays distinct from an unset mode when a Bot is shown or saved', () => {
  const asking = bot({ permissionMode: 'default' });
  const draft = draftFromBot(asking, 'persona');
  assert.equal(draft.permissionMode, 'default');
  assert.deepEqual(botChanges(asking, 'persona', draft).patch, {});
  assert.equal(draftFromBot(bot(), '').permissionMode, '');
  assert.equal(draftFromBot(bot({ permissionMode: 'plan' }), '').permissionMode, 'plan');
  assert.deepEqual(botChanges(bot(), '', { ...draftFromBot(bot(), ''), permissionMode: 'default' }).patch, { permissionMode: 'default' });
  assert.deepEqual(botChanges(asking, 'persona', { ...draft, permissionMode: '' }).patch, { permissionMode: null });
});

test('only what the owner changed is sent, and clearing a choice sends null', () => {
  const current = bot({ model: 'builtin/small', permissionMode: 'plan', workspaces: [{ path: '/work/a', mode: 'read' }] });
  const draft = draftFromBot(current, 'old');
  assert.deepEqual(botChanges(current, 'old', draft), { patch: {}, personaChanged: false });
  assert.deepEqual(botChanges(current, 'old', { ...draft, name: ' Renamed ', description: 'new' }).patch, { name: 'Renamed', description: 'new' });
  assert.deepEqual(botChanges(current, 'old', { ...draft, model: '' }).patch, { model: null });
  assert.deepEqual(botChanges(current, 'old', { ...draft, permissionMode: '' }).patch, { permissionMode: null });
  assert.deepEqual(botChanges(current, 'old', { ...draft, permissionMode: 'acceptEdits' }).patch, { permissionMode: 'acceptEdits' });
  assert.deepEqual(botChanges(current, 'old', { ...draft, workspaces: [{ path: '/work/a', mode: 'write' }] }).patch, { workspaces: [{ path: '/work/a', mode: 'write' }] });
  assert.deepEqual(botChanges(current, 'old', { ...draft, persona: 'new' }), { patch: {}, personaChanged: true });
});

test('the Remote Control switch is shown as the Bot has it, and only a change of it is sent', async () => {
  const off = bot();
  const on = bot({ remote: true });
  assert.equal(draftFromBot(off, '').remote, false);
  assert.equal(draftFromBot(on, '').remote, true);
  assert.deepEqual(botChanges(off, '', draftFromBot(off, '')), { patch: {}, personaChanged: false });
  assert.deepEqual(botChanges(on, '', draftFromBot(on, '')), { patch: {}, personaChanged: false });
  assert.deepEqual(botChanges(off, '', { ...draftFromBot(off, ''), remote: true }).patch, { remote: true });
  assert.deepEqual(botChanges(on, '', { ...draftFromBot(on, ''), remote: false }).patch, { remote: false }, 'off is sent as false, not left out');
  assert.deepEqual(botChanges(on, '', { ...draftFromBot(on, ''), name: 'Renamed' }).patch, { name: 'Renamed' }, 'a switch that stays on is not sent again');
  const { api, calls } = fakeApi();
  const saved = await saveBot(api, off, '', { ...draftFromBot(off, ''), remote: true });
  assert.equal(saved.ok, true);
  assert.deepEqual(calls.update, [['b1', 3, { remote: true }]], 'with the revision it was read at, like every change');
});

test('the sandbox is shown as the Bot has it, and only a change of it is sent', async () => {
  const on = bot({ sandbox: { enabled: true, allowedDomains: ['example.com'] } });
  const off = bot({ sandbox: { enabled: false, allowedDomains: [] } });
  assert.deepEqual(draftFromBot(on, '').sandbox, { enabled: true, allowedDomains: ['example.com'] });
  assert.deepEqual(draftFromBot(off, '').sandbox, { enabled: false, allowedDomains: [] });
  assert.notEqual(draftFromBot(on, '').sandbox.allowedDomains, on.sandbox.allowedDomains, 'the draft holds its own list');
  assert.deepEqual(botChanges(on, '', draftFromBot(on, '')), { patch: {}, personaChanged: false });
  assert.deepEqual(botChanges(off, '', draftFromBot(off, '')), { patch: {}, personaChanged: false });
  assert.deepEqual(botChanges(on, '', { ...draftFromBot(on, ''), sandbox: { enabled: false, allowedDomains: ['example.com'] } }).patch, { sandbox: { enabled: false, allowedDomains: ['example.com'] } }, 'off is sent with the hosts it keeps');
  assert.deepEqual(botChanges(off, '', { ...draftFromBot(off, ''), sandbox: { enabled: true, allowedDomains: [] } }).patch, { sandbox: { enabled: true, allowedDomains: [] } });
  assert.deepEqual(botChanges(on, '', { ...draftFromBot(on, ''), sandbox: { enabled: true, allowedDomains: ['example.com', '*.example.org'] } }).patch, { sandbox: { enabled: true, allowedDomains: ['example.com', '*.example.org'] } });
  assert.deepEqual(botChanges(on, '', { ...draftFromBot(on, ''), sandbox: { enabled: true, allowedDomains: [] } }).patch, { sandbox: { enabled: true, allowedDomains: [] } }, 'an emptied list is sent as empty');
  assert.deepEqual(botChanges(on, '', { ...draftFromBot(on, ''), name: 'Renamed' }).patch, { name: 'Renamed' }, 'a sandbox that stays as it is is not sent again');
  const { api, calls } = fakeApi();
  const saved = await saveBot(api, on, '', { ...draftFromBot(on, ''), sandbox: { enabled: false, allowedDomains: [] } });
  assert.equal(saved.ok, true);
  assert.deepEqual(calls.update, [['b1', 3, { sandbox: { enabled: false, allowedDomains: [] } }]], 'with the revision it was read at, like every change');
});

test('a new Bot is made with the sandbox on and no host, and the request carries the sandbox only when the owner changed it', () => {
  assert.deepEqual(emptyBotDraft().sandbox, { enabled: true, allowedDomains: [] });
  assert.ok(!('sandbox' in botCreateRequest({ ...emptyBotDraft(), name: 'Scribe' })), 'the default is the main process\'s to give');
  assert.deepEqual(botCreateRequest({ ...emptyBotDraft(), name: 'Scribe', sandbox: { enabled: false, allowedDomains: [] } }).sandbox, { enabled: false, allowedDomains: [] });
  assert.deepEqual(botCreateRequest({ ...emptyBotDraft(), name: 'Scribe', sandbox: { enabled: true, allowedDomains: ['example.com'] } }).sandbox, { enabled: true, allowedDomains: ['example.com'] });
  const draft = { ...emptyBotDraft(), name: 'Scribe', sandbox: { enabled: true, allowedDomains: ['example.com'] } };
  assert.notEqual(botCreateRequest(draft).sandbox!.allowedDomains, draft.sandbox.allowedDomains, 'the request holds its own list');
});

test('a host is added the way the main process keeps it, once, and no more than it may hold', () => {
  assert.deepEqual(withAllowedHost([], 'example.com'), { ok: true, domains: ['example.com'] });
  assert.deepEqual(withAllowedHost(['example.com'], '  *.Example.ORG '), { ok: true, domains: ['example.com', '*.example.org'] }, 'trimmed and in lower case, after the ones there are');
  const before = ['example.com'];
  withAllowedHost(before, 'example.org');
  assert.deepEqual(before, ['example.com'], 'the list it was given is left as it was');
  assert.deepEqual(withAllowedHost(['example.com'], 'EXAMPLE.com'), { ok: false, problem: 'duplicate' });
  for (const text of ['', '   ', 'https://example.com', 'example.com/path', 'example.com:8080', 'exa mple.com', '*', '*.', '*example.com', 'a..b', '-a.com', 'a-.com', '*.*.com', 'user@example.com', 'x'.repeat(64) + '.com', `${'a.'.repeat(130)}com`]) {
    assert.deepEqual(withAllowedHost([], text), { ok: false, problem: 'invalid' }, JSON.stringify(text));
  }
  const full = Array.from({ length: BOT_ALLOWED_DOMAINS_MAX }, (_, index) => `host${index}.example.com`);
  assert.deepEqual(withAllowedHost(full.slice(1), 'last.example.com').ok, true, 'room for one more');
  assert.deepEqual(withAllowedHost(full, 'last.example.com'), { ok: false, problem: 'too_many' });
  assert.deepEqual(withAllowedHost(full, 'host3.example.com'), { ok: false, problem: 'duplicate' }, 'a host that is there is reported as that, not as the list being full');
  // What the main process would keep is what this keeps.
  assert.equal(allowedDomain('  Example.COM '), 'example.com');
  assert.equal(allowedDomain('*.example.com'), '*.example.com');
  assert.equal(allowedDomain('localhost'), 'localhost');
});

test('more hosts than the main process allows is a problem of the form before anything is sent', () => {
  const draft = (count: number): BotDraft => ({ ...emptyBotDraft(), name: 'Scribe', sandbox: { enabled: true, allowedDomains: Array.from({ length: count }, (_, index) => `host${index}.example.com`) } });
  assert.deepEqual(botDraftProblems(draft(BOT_ALLOWED_DOMAINS_MAX)), {});
  assert.deepEqual(botDraftProblems(draft(BOT_ALLOWED_DOMAINS_MAX + 1)), { sandbox: 'sandbox_too_many' });
});

test('a Bot refused for want of a sandbox is known by the words of the refusal, however they arrive', () => {
  assert.equal(isBotSandboxUnavailable(new Error(BOT_SANDBOX_UNAVAILABLE_MESSAGE)), true);
  assert.equal(isBotSandboxUnavailable(overIpc('lingxi:bots:sessions:open', BOT_SANDBOX_UNAVAILABLE_MESSAGE)), true);
  assert.equal(isBotSandboxUnavailable(overIpc('lingxi:bots:sessions:open', 'The Bot\'s folder is missing.')), false);
  assert.equal(isBotSandboxUnavailable(undefined), false);
});

test('saving writes the record, then the persona, and does nothing when nothing changed', async () => {
  const current = bot();
  const { api, calls } = fakeApi();
  const unchanged = await saveBot(api, current, 'persona', draftFromBot(current, 'persona'));
  assert.equal(unchanged.ok, true);
  assert.deepEqual(calls, { update: [], persona: [], create: [] });

  const result = await saveBot(api, current, 'persona', { ...draftFromBot(current, 'persona'), name: 'Renamed', persona: 'changed' });
  assert.equal(result.ok, true);
  assert.equal(result.ok && result.bot.name, 'Renamed');
  assert.equal(result.ok && result.persona, 'changed');
  // The record is sent with the revision it was read at; the persona follows it.
  assert.deepEqual(calls.update, [['b1', 3, { name: 'Renamed' }]]);
  assert.deepEqual(calls.persona, [['b1', 'changed']]);
});

test('a Bot that changed elsewhere is a conflict: nothing is overwritten, however the error arrives', async () => {
  for (const error of [new Error(BOT_CONFLICT_MESSAGE), overIpc('lingxi:bots:update', BOT_CONFLICT_MESSAGE)]) {
    const current = bot();
    const { api, calls } = fakeApi({ update: () => error });
    const result = await saveBot(api, current, 'persona', { ...draftFromBot(current, 'persona'), name: 'Renamed', persona: 'changed' });
    assert.deepEqual(result, { ok: false, conflict: true });
    // The persona is written after the record, so a conflict leaves it as it was.
    assert.equal(calls.persona.length, 0);
  }
});

test('other failures come back as words, and a record that was written is not lost when the persona was not', async () => {
  const current = bot();
  const draft = { ...draftFromBot(current, 'persona'), name: 'Renamed', persona: 'changed' };

  const refused = fakeApi({ update: () => overIpc('lingxi:bots:update', 'The name is taken.') });
  assert.deepEqual(await saveBot(refused.api, current, 'persona', draft), { ok: false, conflict: false, message: 'The name is taken.' });

  const half = fakeApi({ persona: overIpc('lingxi:bots:persona:set', 'The persona could not be written.') });
  const result = await saveBot(half.api, current, 'persona', draft);
  assert.equal(result.ok, false);
  assert.equal(!result.ok && !result.conflict && result.message, 'The persona could not be written.');
  assert.equal(!result.ok && !result.conflict && result.saved?.revision, 4);

  const problems = fakeApi();
  assert.deepEqual(await saveBot(problems.api, current, 'persona', { ...draft, name: '' }), { ok: false, conflict: false, message: '' });
  assert.equal(problems.calls.update.length, 0);
});

test('the list puts the most recently used Bot first', () => {
  const bots = [
    bot({ botId: 'a', name: 'Alpha', createdAt: 100 }),
    bot({ botId: 'b', name: 'Beta', createdAt: 100, lastActivityAt: 900 }),
    bot({ botId: 'c', name: 'Gamma', createdAt: 500 }),
    bot({ botId: 'd', name: 'Delta', createdAt: 500 }),
  ];
  assert.deepEqual(sortBots(bots).map((entry) => entry.botId), ['b', 'd', 'c', 'a']);
  assert.deepEqual(bots.map((entry) => entry.botId), ['a', 'b', 'c', 'd']);
});

test('a folder is added once, read-only unless said otherwise', () => {
  const folders = withFolder([], '/work/a');
  assert.deepEqual(folders, [{ path: '/work/a', mode: 'read' }]);
  assert.deepEqual(withFolder([{ path: '/work/a', mode: 'write' }], '/work/a'), [{ path: '/work/a', mode: 'write' }]);
  assert.deepEqual(withFolder(folders, '/work/b', 'write'), [{ path: '/work/a', mode: 'read' }, { path: '/work/b', mode: 'write' }]);
  assert.deepEqual(folders, [{ path: '/work/a', mode: 'read' }]);
});

test('a folder carries the name the owner gave it, trimmed, and no name when there is none', () => {
  const saved = bot({ workspaces: [{ id: 'wA', path: '/work/a', mode: 'read', addedAt: 5, label: 'Notes' }, { id: 'wB', path: '/work/b', mode: 'write', addedAt: 6 }] });
  const draft = draftFromBot(saved, '');
  // The draft holds what the owner edits, not what the main process keeps (the id and the time the folder was added).
  assert.deepEqual(draft.workspaces, [{ path: '/work/a', mode: 'read', label: 'Notes' }, { path: '/work/b', mode: 'write' }]);
  assert.deepEqual(botChanges(saved, '', draft).patch, {});
  // Spaces around a name are no change, and a name of spaces is no name.
  const spaced = { ...draft, workspaces: [{ ...draft.workspaces[0]!, label: '  Notes ' }, { ...draft.workspaces[1]!, label: '   ' }] };
  assert.deepEqual(botChanges(saved, '', spaced).patch, {});
  const renamed = { ...draft, workspaces: [{ ...draft.workspaces[0]!, label: ' Reading ' }, draft.workspaces[1]!] };
  assert.deepEqual(botChanges(saved, '', renamed).patch, { workspaces: [{ path: '/work/a', mode: 'read', label: 'Reading' }, { path: '/work/b', mode: 'write' }] });
  const cleared = { ...draft, workspaces: [{ path: '/work/a', mode: 'read' as const, label: '' }, draft.workspaces[1]!] };
  assert.deepEqual(botChanges(saved, '', cleared).patch, { workspaces: [{ path: '/work/a', mode: 'read' }, { path: '/work/b', mode: 'write' }] });
  const made = botCreateRequest({ ...emptyBotDraft(), name: 'Scribe', workspaces: [{ path: '/work/a', mode: 'read', label: ' Notes ' }, { path: '/work/b', mode: 'write', label: ' ' }] });
  assert.deepEqual(made.workspaces, [{ path: '/work/a', mode: 'read', label: 'Notes' }, { path: '/work/b', mode: 'write' }]);
});

test('a name for a folder may have as many characters as the main process allows', () => {
  const draft = (label: string): BotDraft => ({ ...emptyBotDraft(), name: 'Scribe', workspaces: [{ path: '/work/a', mode: 'read', label }] });
  assert.deepEqual(botDraftProblems(draft('x'.repeat(80))), {});
  assert.deepEqual(botDraftProblems(draft(`  ${'x'.repeat(80)}  `)), {});
  assert.deepEqual(botDraftProblems(draft('x'.repeat(81))), { folders: 'folders_label_too_long' });
});

test('folders bound to the Bot while the owner edits it are added to the draft, which is otherwise left as it is', () => {
  const before = bot({ workspaces: [{ id: 'wA', path: '/work/a', mode: 'read', addedAt: 5 }] });
  const after = bot({ revision: 4, workspaces: [...before.workspaces, { id: 'wB', path: '/work/b', mode: 'write', addedAt: 9, label: 'Specs' }] });
  const editing: BotDraft = { ...draftFromBot(before, 'persona'), name: 'Renamed', workspaces: [{ path: '/work/a', mode: 'write' }, { path: '/work/c', mode: 'read' }] };
  const next = draftWithNewFolders(editing, before, after);
  assert.equal(next.name, 'Renamed');
  assert.deepEqual(next.workspaces, [{ path: '/work/a', mode: 'write' }, { path: '/work/c', mode: 'read' }, { path: '/work/b', mode: 'write', label: 'Specs' }]);
  // A folder the owner added to the draft by hand is not added twice, nor is one they took out brought back.
  const same = draftWithNewFolders({ ...editing, workspaces: [{ path: '/work/b', mode: 'read' }] }, before, after);
  assert.deepEqual(same.workspaces, [{ path: '/work/b', mode: 'read' }]);
  assert.equal(draftWithNewFolders(editing, before, before), editing, 'nothing new: the draft itself');
});
