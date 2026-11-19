import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SessionRuntime } from '../src/main/bridge.js';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { SettingsStore } from '../src/main/settings';
import type { ClientCommand } from '@lingxi/bridge-client';
import type { ProviderAccountLaunch } from '../src/main/provider-accounts.js';
const defaultRef = { projectPath: '/workspace', sessionId: '11111111-2222-4333-8444-555555555555' };
function runtime(store: SettingsStore, ref = defaultRef, resolveProviderAccounts?: () => Promise<ProviderAccountLaunch | undefined>) {
  const commands: ClientCommand[] = [];
  const observed: string[] = [];
  const client = Object.assign(new EventEmitter(), { cancel: () => undefined,
    getWorkspaceState: async () => ({ cwd: ref.projectPath, additional_directories: [] as string[] }),
    sendCommand: (command: ClientCommand) => {
    // Audio context refreshes are independent of model persistence.
    if (command.type !== 'get_audio_session_context') commands.push(command);
  } });
  const instance = new SessionRuntime({
    ...ref,
    expectedSessionMode: 'code',
    launchConfig: () => ({ workspace: '/workspace', trusted: true, model: store.getPublic().model }),
    onModelSelected: model => store.setSessionModel(ref, model),
    getSavedModel: () => store.getSessionModel(ref),
    onModelChanged: model => observed.push(model),
    resolveProviderAccounts,
  });
  Object.assign(instance, { client, activeWorkspace: '/workspace', activeWorkspaceTrusted: true });
  (instance as any).wireClient(client, 0);
  return { instance, client, commands, observed };
}
const tick = () => new Promise(resolve => setImmediate(resolve));
function modelPreferenceCommands(commands: ClientCommand[]): ClientCommand[] {
  // model_changed now refreshes the session's audio context as a separate
  // runtime concern; these tests assert model preference effects only.
  return commands.filter(command => command.type !== 'get_audio_session_context');
}

function aliasFence(client: ReturnType<typeof runtime>['client']) {
  const releases: Array<() => void> = [];
  client.getWorkspaceState = () => new Promise(resolve => {
    releases.push(() => resolve({ cwd: defaultRef.projectPath, additional_directories: [] }));
  });
  return releases;
}

test('direct alias ACK uses exact UTF16 source and a correlated FIFO fence before saving the canonical model', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-source-ack-'));
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const { instance, client } = runtime(store);
  const fences = aliasFence(client);
  const source = 'release-\ud800';
  const selecting = instance.dispatchCommand({ type: 'set_model', model: source });
  const settled = Promise.allSettled([selecting]);
  try {
    await tick();
    client.emit('event', { type: 'model_changed', model: 'provider-a/concrete-model', model_source: 'release-\ufffd' });
    await tick();
    assert.equal(fences.length, 0, 'a lossy replacement is not the requested source');
    client.emit('event', { type: 'model_changed', model: 'provider-a/concrete-model', model_source: source });
    await tick();
    assert.equal(fences.length, 1, 'source aliases require a correlated command-queue fence');
    client.emit('event', { type: 'model_list', current: 'provider-a/concrete-model', models: ['provider-a/concrete-model'] });
    await tick();
    assert.equal(store.getPublic().model, 'old-model', 'unsolicited model listings cannot release that fence');
    fences.shift()!();
    assert.equal((await settled)[0].status, 'fulfilled');
    assert.equal(store.getPublic().model, 'provider-a/concrete-model');
  } finally {
    instance.cancelTurn(undefined); fences.splice(0).forEach(release => release());
    await settled; rmSync(dir, { recursive: true, force: true });
  }
});

test('direct model ACK cannot use another provider or a missing or wrong source as a suffix match', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  for (const requested of ['shared-model', 'provider-a/shared-model']) {
    const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-ack-scope-'));
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client } = runtime(store);
    client.emit('event', { type: 'settings_snapshot', effective_json: JSON.stringify({ providers: {
      'provider-a': { models: ['shared-model'] }, 'provider-b': { models: ['shared-model'] },
    } }), provenance_json: '{}' });
    const selecting = instance.dispatchCommand({ type: 'set_model', model: requested });
    const settled = Promise.allSettled([selecting]);
    try {
      await tick();
      for (const source of [undefined, 'wrong-source', ...(requested.includes('/') ? [requested] : [])]) {
        client.emit('event', { type: 'model_changed', model: 'provider-b/shared-model', ...(source === undefined ? {} : { model_source: source }) });
        client.emit('event', { type: 'model_list', current: 'provider-b/shared-model', models: ['provider-a/shared-model', 'provider-b/shared-model'] });
        await tick();
        assert.equal(store.getPublic().model, 'old-model');
        assert.equal(instance.turnActive, true, 'an unrelated provider/source cannot confirm the pending selection');
      }
    } finally {
      instance.cancelTurn(undefined); await settled;
      rmSync(dir, { recursive: true, force: true });
    }
  }
});

test('a bare wire id containing a slash is not mistaken for an explicit provider scope', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-nested-wire-'));
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const { instance, client } = runtime(store);
  const fences = aliasFence(client);
  client.emit('event', { type: 'settings_snapshot', effective_json: JSON.stringify({ providers: {
    'provider-a': { models: ['vendor/shared-model'] },
  } }), provenance_json: '{}' });
  const selecting = instance.dispatchCommand({ type: 'set_model', model: 'vendor/shared-model' });
  const settled = Promise.allSettled([selecting]);
  try {
    await tick();
    client.emit('event', { type: 'model_changed', model: 'provider-a/vendor/shared-model', model_source: 'vendor/shared-model' });
    await tick(); assert.equal(fences.length, 1); fences.shift()!();
    assert.equal((await settled)[0].status, 'fulfilled');
    assert.equal(store.getPublic().model, 'provider-a/vendor/shared-model');
  } finally {
    instance.cancelTurn(undefined); fences.splice(0).forEach(release => release());
    await settled; rmSync(dir, { recursive: true, force: true });
  }
});

test('an alias fence failure cannot save a selection or leave its pending switch busy', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-fence-error-'));
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const { instance, client } = runtime(store);
  let fences = 0;
  client.getWorkspaceState = async () => { fences++; throw new Error('fixture connection closed'); };
  const selecting = instance.dispatchCommand({ type: 'set_model', model: 'latest' });
  const settled = Promise.allSettled([selecting]);
  try {
    await tick();
    client.emit('event', { type: 'model_changed', model: 'provider-a/current-model', model_source: 'latest' });
    await tick(); assert.equal(fences, 1);
    assert.equal((await settled)[0].status, 'rejected');
    assert.equal(instance.turnActive, false);
    assert.equal(store.getPublic().model, 'old-model');
  } finally {
    instance.cancelTurn(undefined); await settled;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('a cancelled alias fence cannot persist over a fresh confirmed selection when it resolves later', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-stale-fence-'));
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const { instance, client } = runtime(store);
  const fences = aliasFence(client);
  const first = instance.dispatchCommand({ type: 'set_model', model: 'latest' });
  const firstOutcome = Promise.allSettled([first]);
  let secondOutcome: Promise<PromiseSettledResult<void>[]> | undefined;
  try {
    await tick(); client.emit('event', { type: 'model_changed', model: 'provider-a/old-model', model_source: 'latest' });
    await tick(); assert.equal(fences.length, 1);
    instance.cancelTurn(undefined); await firstOutcome;
    const second = instance.dispatchCommand({ type: 'set_model', model: 'provider-a/fresh-model' });
    secondOutcome = Promise.allSettled([second]); await tick();
    client.emit('event', { type: 'model_changed', model: 'provider-a/fresh-model' });
    assert.equal((await secondOutcome)[0].status, 'fulfilled');
    fences.shift()!(); await tick();
    assert.equal(store.getPublic().model, 'provider-a/fresh-model');
  } finally {
    instance.cancelTurn(undefined); fences.splice(0).forEach(release => release());
    await firstOutcome; await secondOutcome;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cancel then the same alias ignores old ACK/listing and confirms only the latest raw ACK after its fence', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-alias-cancel-'));
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const { instance, client, commands } = runtime(store);
  const fences = aliasFence(client);
  const first = instance.dispatchCommand({ type: 'set_model', model: 'latest' });
  const firstOutcome = Promise.allSettled([first]);
  let secondOutcome: Promise<PromiseSettledResult<void>[]> | undefined;
  try {
    await tick(); instance.cancelTurn(undefined); await firstOutcome;
    const second = instance.dispatchCommand({ type: 'set_model', model: 'latest' });
    secondOutcome = Promise.allSettled([second]); await tick();
    assert.equal(commands.filter(command => command.type === 'set_model').length, 2);
    client.emit('event', { type: 'model_changed', model: 'provider-a/old-version', model_source: 'latest' });
    await tick();
    assert.equal(fences.length, 1);
    client.emit('event', { type: 'model_list', current: 'provider-a/old-version', models: ['provider-a/old-version'] });
    await tick();
    assert.equal(store.getPublic().model, 'old-model');
    client.emit('event', { type: 'model_changed', model: 'provider-a/new-version', model_source: 'latest' });
    await tick();
    fences.splice(0).forEach(release => release());
    assert.equal((await secondOutcome)[0].status, 'fulfilled');
    assert.equal(store.getPublic().model, 'provider-a/new-version', 'the fence must use the latest raw ACK, not its earlier candidate');
  } finally {
    instance.cancelTurn(undefined); fences.splice(0).forEach(release => release());
    await firstOutcome; await secondOutcome;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('credential injection ACK is a real FIFO barrier before the replacement model command is marked sent', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-injection-fifo-'));
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const launch: ProviderAccountLaunch = { providerAccounts: { storeScope: 'fixture', configRevision: 1, accounts: [], health: [] }, providerAccountCredentials: {} };
  const { instance, client, commands } = runtime(store, defaultRef, async () => launch);
  Object.assign(instance, { state: { status: 'connected' } });
  const fences = aliasFence(client);
  const first = instance.dispatchCommand({ type: 'set_model', model: 'latest' });
  const firstOutcome = Promise.allSettled([first]);
  let secondOutcome: Promise<PromiseSettledResult<void>[]> | undefined;
  const injects = () => commands.filter((command): command is Extract<ClientCommand, { type: 'provider_accounts' }> => command.type === 'provider_accounts');
  try {
    await tick();
    assert.equal(commands.filter(command => command.type === 'set_model').length, 0);
    client.emit('event', { type: 'provider_accounts_updated', operation_id: injects()[0].operation_id, snapshot_json: '{}' });
    await tick(); instance.cancelTurn(undefined); await firstOutcome;
    const second = instance.dispatchCommand({ type: 'set_model', model: 'latest' });
    secondOutcome = Promise.allSettled([second]); await tick();
    assert.equal(injects().length, 2);
    assert.equal((instance as any).pendingModelSwitch.sent, false);
    client.emit('event', { type: 'model_changed', model: 'provider-a/old-version', model_source: 'latest' });
    await tick();
    assert.equal(fences.length, 0, 'the old ACK is before the injection ACK, so the new model has not been sent');
    assert.equal(commands.filter(command => command.type === 'set_model').length, 1);
    client.emit('event', { type: 'provider_accounts_updated', operation_id: injects()[1].operation_id, snapshot_json: '{}' });
    await tick();
    assert.equal((instance as any).pendingModelSwitch.sent, true);
    assert.equal(commands.filter(command => command.type === 'set_model').length, 2);
    client.emit('event', { type: 'model_changed', model: 'provider-a/new-version', model_source: 'latest' });
    await tick();
    assert.equal(fences.length, 1); fences.shift()!();
    assert.equal((await secondOutcome)[0].status, 'fulfilled');
    assert.equal(store.getPublic().model, 'provider-a/new-version');
  } finally {
    instance.cancelTurn(undefined); fences.splice(0).forEach(release => release());
    for (const command of injects()) client.emit('event', { type: 'provider_accounts_updated', operation_id: command.operation_id, snapshot_json: '{}' });
    await firstOutcome; await secondOutcome;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('model confirmation gives the full ACK budget after waiting for credentials', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-credential-budget-'));
  let release!: () => void;
  const credential = new Promise<void>(resolve => { release = resolve; });
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const { instance, client, commands } = runtime(store, defaultRef, async () => undefined);
  instance.refreshProviderAccounts = () => credential;
  let outcome = 'pending';
  const selection = instance.dispatchCommand({ type: 'set_model', model: 'new-model' });
  const settled = selection.then(() => { outcome = 'confirmed'; }, () => { outcome = 'failed'; });
  try {
    t.mock.timers.tick(10_001);
    await tick();
    assert.equal(outcome, 'pending', 'credential preparation must not consume the model ACK timeout');
    assert.deepEqual(modelPreferenceCommands(commands), []);
    assert.equal(store.getPublic().model, 'old-model');
    release();
    await tick();
    assert.deepEqual(modelPreferenceCommands(commands), [{ type: 'set_model', model: 'new-model' }]);
    t.mock.timers.tick(9_999);
    await tick();
    client.emit('event', { type: 'model_changed', model: 'another-model' });
    await tick();
    assert.equal(outcome, 'pending', 'only the requested model can acknowledge the switch');
    client.emit('event', { type: 'model_changed', model: 'new-model' });
    await selection;
    assert.equal(store.getPublic().model, 'new-model');
  } finally {
    release();
    instance.cancelTurn(undefined);
    await settled;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('model confirmation still times out after dispatch and ignores a late ACK', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-ack-deadline-'));
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const { instance, client, commands } = runtime(store, defaultRef, async () => undefined);
  instance.refreshProviderAccounts = async () => undefined;
  try {
    const selection = instance.dispatchCommand({ type: 'set_model', model: 'new-model' });
    const rejected = assert.rejects(selection, /Model switch confirmation timed out/);
    await tick();
    assert.deepEqual(modelPreferenceCommands(commands), [{ type: 'set_model', model: 'new-model' }]);
    t.mock.timers.tick(10_000);
    await rejected;
    client.emit('event', { type: 'model_changed', model: 'new-model' });
    assert.equal(store.getPublic().model, 'old-model', 'late ACK must not save a failed selection');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('a model switch requested during a long compact is confirmed before the pending prompt is dispatched', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-during-compact-'));
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const { instance, client, commands } = runtime(store, defaultRef, async () => undefined);
  const dispatched: Array<{ text: string; model: string }> = [];
  instance.refreshProviderAccounts = async () => undefined;
  client.emit('event', { type: 'settings_snapshot', effective_json: '{}', provenance_json: '{}' });
  Object.assign(client, { sendPrompt: (text: string) => dispatched.push({
    text, model: store.getPublic().model,
  }) });
  await instance.dispatchCommand({ type: 'force_compact' });
  client.emit('event', { type: 'compaction_status', phase: 'preparing' });
  const earlierPrompt = instance.sendPrompt('already pending');
  const selection = instance.dispatchCommand({ type: 'set_model', model: 'new-model' });
  const prompt = instance.sendPrompt('follow up');
  const settled = Promise.allSettled([earlierPrompt, selection, prompt]);
  try {
    await tick();
    t.mock.timers.tick(90_000);
    await tick();
    assert.deepEqual(modelPreferenceCommands(commands), [{ type: 'force_compact' }]);
    assert.equal(store.getPublic().model, 'old-model');
    assert.deepEqual(dispatched, []);
    client.emit('event', { type: 'compaction_status', phase: 'error', error: 'Summary failed' });
    client.emit('event', { type: 'error', kind: { type: 'internal' }, message: 'force_compact failed: Summary failed' });
    await tick();
    assert.deepEqual(modelPreferenceCommands(commands), [{ type: 'force_compact' }, { type: 'set_model', model: 'new-model' }]);
    t.mock.timers.tick(9_999);
    client.emit('event', { type: 'model_changed', model: 'new-model' });
    await Promise.all([earlierPrompt, selection, prompt]);
    assert.deepEqual(dispatched, [
      { text: 'already pending', model: 'new-model' },
      { text: 'follow up', model: 'new-model' },
    ]);
  } finally {
    client.emit('event', { type: 'compaction_status', phase: 'cancelled' });
    instance.cancelTurn(undefined);
    await settled;
    rmSync(dir, { recursive: true, force: true });
  }
});

test('cancelling a slow credential wait permits a fresh switch without sending the stale selection', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-cancel-credential-'));
  let release!: () => void;
  const credential = new Promise<void>(resolve => { release = resolve; });
  const store = new SettingsStore(dir); store.setLastModel('old-model');
  const { instance, client, commands } = runtime(store, defaultRef, async () => undefined);
  instance.refreshProviderAccounts = () => credential;
  const first = instance.dispatchCommand({ type: 'set_model', model: 'cancelled-model' });
  const firstOutcome = Promise.allSettled([first]);
  let secondOutcome: Promise<PromiseSettledResult<void>[]> | undefined;
  try {
    t.mock.timers.tick(10_001);
    await tick();
    instance.cancelTurn(undefined);
    const [cancelled] = await firstOutcome;
    assert.equal(cancelled.status, 'rejected');
    if (cancelled.status === 'rejected') assert.match(String(cancelled.reason), /Model switch was cancelled/);
    instance.refreshProviderAccounts = async () => undefined;
    const second = instance.dispatchCommand({ type: 'set_model', model: 'fresh-model' });
    secondOutcome = Promise.allSettled([second]);
    release();
    await tick();
    assert.deepEqual(modelPreferenceCommands(commands), [{ type: 'set_model', model: 'fresh-model' }]);
    client.emit('event', { type: 'model_changed', model: 'cancelled-model' });
    assert.equal(store.getPublic().model, 'old-model');
    client.emit('event', { type: 'model_changed', model: 'fresh-model' });
    assert.equal((await secondOutcome)[0].status, 'fulfilled');
    assert.equal(store.getPublic().model, 'fresh-model');
  } finally {
    release();
    instance.cancelTurn(undefined);
    await firstOutcome;
    await secondOutcome;
    rmSync(dir, { recursive: true, force: true });
  }
});
test('only confirmed user model selections replace the restart default; history still updates observers', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client, commands, observed } = runtime(store);
    client.emit('event', { type: 'model_changed', model: 'historical-model' });
    assert.equal(store.getPublic().model, 'old-model');
    const selection = instance.dispatchCommand({ type: 'set_model', model: 'new-model' });
    await tick();
    assert.deepEqual(modelPreferenceCommands(commands), [{ type: 'set_model', model: 'new-model' }]);
    assert.equal(store.getPublic().model, 'old-model');
    client.emit('event', { type: 'model_changed', model: 'unrelated-model' });
    assert.equal(store.getPublic().model, 'old-model');
    client.emit('event', { type: 'model_changed', model: 'new-model' });
    await selection;
    client.emit('event', { type: 'model_changed', model: 'historical-model' });
    assert.equal(new SettingsStore(dir).getPublic().model, 'new-model');
    assert.deepEqual(observed, ['historical-model', 'unrelated-model', 'new-model', 'historical-model']);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('failed and interrupted model switches retain the saved choice', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client } = runtime(store);
    const selection = instance.dispatchCommand({ type: 'set_model', model: 'invalid-model' });
    await tick();
    client.emit('event', { type: 'error', kind: { type: 'internal' }, message: 'switch_model failed: unknown model' });
    await assert.rejects(selection, /Model switch failed/);
    const interrupted = instance.dispatchCommand({ type: 'set_model', model: 'new-model' });
    await tick();
    (instance as any).setState({ status: 'disconnected', reason: 'closed' });
    await assert.rejects(interrupted, /interrupted/);
    assert.equal(new SettingsStore(dir).getPublic().model, 'old-model');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('write failure is reported and rolls back the restart default', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const before = readFileSync(store.settingsPath, 'utf8');
    mkdirSync(`${store.settingsPath}.tmp`);
    const { instance, client, observed } = runtime(store);
    const selection = instance.dispatchCommand({ type: 'set_model', model: 'new-model' });
    await tick();
    client.emit('event', { type: 'model_changed', model: 'new-model' });
    await assert.rejects(selection, /Could not save model/);
    assert.equal(store.getPublic().model, 'old-model');
    assert.equal(readFileSync(store.settingsPath, 'utf8'), before);
    assert.equal(store.getSessionModel(defaultRef), undefined);
    assert.deepEqual(observed, ['new-model']);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('resume reapplies the last selection over an old snapshot even without a subsequent message', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    new SettingsStore(dir).setSessionModel(defaultRef, 'latest-model');
    const store = new SettingsStore(dir);
    const { instance, client, commands } = runtime(store);
    const resumed = instance.resumeOwnedSession();
    client.emit('event', { type: 'session_resumed', session_id: instance.sessionId, mode: 'code', messages: [] });
    client.emit('event', { type: 'model_changed', model: 'historical-model' });
    await tick();
    const preferenceCommands = modelPreferenceCommands(commands);
    assert.deepEqual(preferenceCommands.slice(0, 2).map(command => command.type), ['resume_session', 'set_model']);
    assert.deepEqual(preferenceCommands[1], { type: 'set_model', model: 'latest-model' });
    client.emit('event', { type: 'model_changed', model: 'latest-model' });
    await resumed;
    assert.equal(new SettingsStore(dir).getPublic().model, 'latest-model');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('a now-unavailable saved model reports restore failure without breaking resume or overwriting it', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setSessionModel(defaultRef, 'unavailable-model');
    const { instance, client } = runtime(store);
    const events: unknown[] = [];
    (instance as any).broadcastClientEvent = (event: unknown) => events.push(event);
    const resumed = instance.resumeOwnedSession();
    client.emit('event', { type: 'session_resumed', session_id: instance.sessionId, mode: 'code', messages: [] });
    client.emit('event', { type: 'model_changed', model: 'historical-model' });
    await tick();
    client.emit('event', { type: 'error', kind: { type: 'rejected' }, message: 'unknown model' });
    await resumed;
    assert.equal(store.getPublic().model, 'unavailable-model');
    assert.ok(events.some(event => JSON.stringify(event).includes('Could not restore model')));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('explicit /model persists only on confirmation, including reselecting the current model', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client, commands } = runtime(store);
    const events: any[] = [];
    (instance as any).broadcastClientEvent = (event: unknown) => events.push(event);
    client.emit('event', { type: 'model_changed', model: 'current-model' });
    const selecting = instance.dispatchCommand({ type: 'run_slash_command', raw: '/model current-model ', images: [], turn_id: 123 });
    await tick();
    assert.deepEqual(modelPreferenceCommands(commands), [{ type: 'run_slash_command', raw: '/model current-model ', images: [], turn_id: 123 }]);
    assert.equal(store.getPublic().model, 'old-model');
    assert.ok(!events.some(event => event.type === 'slash_command_result'));
    await assert.rejects(instance.dispatchCommand({ type: 'run_slash_command', raw: '/model', images: [] }), /already in progress/);
    client.emit('event', { type: 'slash_command_result', turn_id: 123, display: 'Switched', is_error: false });
    client.emit('event', { type: 'model_list', current: 'provider/current-model', models: ['provider/current-model'] });
    await selecting;
    assert.equal(new SettingsStore(dir).getPublic().model, 'provider/current-model');
    assert.ok(events.some(event => event.type === 'slash_command_result' && !event.is_error));
    await instance.dispatchCommand({ type: 'run_slash_command', raw: '/model', images: [], turn_id: 124 });
    assert.deepEqual(commands.at(-1), { type: 'run_slash_command', raw: '/model', images: [], turn_id: 124 });
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
test('a late side-question reply cannot complete or reject a pending slash model switch', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client, commands } = runtime(store);
    await instance.dispatchCommand({ type: 'run_slash_command', raw: '/btw progress?', images: [], turn_id: 41 });
    const selecting = instance.dispatchCommand({ type: 'run_slash_command', raw: '/model next-model', images: [], turn_id: 42 });
    await tick();
    for (const is_error of [true, false]) {
      client.emit('event', { type: 'slash_command_result', turn_id: 41, display: 'Side answer', is_error });
    }
    await tick();
    assert.equal(commands.length, 2, 'side replies must not trigger model confirmation');
    assert.ok((instance as any).pendingModelSwitch);
    assert.equal(store.getPublic().model, 'old-model');
    client.emit('event', { type: 'slash_command_result', turn_id: 42, display: 'Switched', is_error: false });
    client.emit('event', { type: 'model_list', current: 'next-model', models: ['next-model'] });
    await selecting;
    assert.equal(store.getPublic().model, 'next-model');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('side-question dispatch and replies preserve main turn ownership', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const { instance, client, commands } = runtime(new SettingsStore(dir));
    client.emit('event', { type: 'turn_started', turn_id: 7 });
    await instance.dispatchCommand({ type: 'run_slash_command', raw: '/btw progress?', images: [], turn_id: 42 });
    assert.deepEqual(commands, [{ type: 'run_slash_command', raw: '/btw progress?', images: [], turn_id: 42 }]);
    for (const is_error of [true, false]) {
      client.emit('event', { type: 'slash_command_result', turn_id: 42, display: 'Side answer', is_error });
      assert.equal(instance.turnActive, true);
      assert.equal((instance as any).activeTurnId, 7);
    }
    client.emit('event', { type: 'turn_ended', turn_id: 7 });
    assert.equal(instance.turnActive, false);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('rejected explicit /model never saves or reports success', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-preference-'));
  try {
    const store = new SettingsStore(dir); store.setLastModel('old-model');
    const { instance, client } = runtime(store);
    const events: any[] = [];
    (instance as any).broadcastClientEvent = (event: unknown) => events.push(event);
    const selecting = instance.dispatchCommand({ type: 'run_slash_command', raw: '/model forbidden-model', images: [], turn_id: 125 });
    await tick();
    client.emit('event', { type: 'slash_command_result', turn_id: 125, display: 'Could not switch model: blocked by hook', is_error: false });
    client.emit('event', { type: 'model_list', current: 'old-model', models: ['old-model'] });
    await assert.rejects(selecting, /not confirmed/);
    assert.equal(new SettingsStore(dir).getPublic().model, 'old-model');

  } finally { rmSync(dir, { recursive: true, force: true }); }
});


test('session model selections remain isolated across resume and settings reload', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-model-isolation-'));
  try {
    const refB = { ...defaultRef, sessionId: '11111111-2222-4333-8444-555555555556' };
    const store = new SettingsStore(dir);
    store.setSessionModel(refB, 'model-b');
    const a = runtime(store);
    const selected = a.instance.dispatchCommand({ type: 'set_model', model: 'model-a' });
    await tick();
    a.client.emit('event', { type: 'model_changed', model: 'model-a' });
    await selected;
    const reloaded = new SettingsStore(dir);
    assert.equal(reloaded.getPublic().model, 'model-a', 'new sessions inherit the last choice');
    assert.equal(reloaded.getSessionModel(defaultRef), 'model-a');
    assert.equal(reloaded.getSessionModel(refB), 'model-b');
    assert.equal(reloaded.getSessionModel({ ...defaultRef, projectPath: '/other' }), undefined);
    for (const saved of [store, reloaded]) {
      const b = runtime(saved, refB);
      const resumed = b.instance.resumeOwnedSession();
      b.client.emit('event', { type: 'session_resumed', session_id: refB.sessionId, mode: 'code', messages: [] });
      b.client.emit('event', { type: 'model_changed', model: 'old-model-b' });
      await tick();
      assert.deepEqual(modelPreferenceCommands(b.commands)[1], { type: 'set_model', model: 'model-b' });
      b.client.emit('event', { type: 'model_changed', model: 'model-b' });
      await resumed;
    }
    const legacy = runtime(reloaded, { ...refB, sessionId: '11111111-2222-4333-8444-555555555557' });
    const resumed = legacy.instance.resumeOwnedSession();
    legacy.client.emit('event', { type: 'session_resumed', session_id: legacy.instance.sessionId, mode: 'code', messages: [] });
    legacy.client.emit('event', { type: 'model_changed', model: 'historical-model' });
    await resumed;
    assert.deepEqual(modelPreferenceCommands(legacy.commands).map(command => command.type), ['resume_session']);
    assert.deepEqual(legacy.observed, ['historical-model']);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('runtime manager binds model preferences to each owning session', async () => {
  const refB = { ...defaultRef, sessionId: '11111111-2222-4333-8444-555555555556' };
  const selections: unknown[] = [];
  const manager = new SessionRuntimeManager({
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    getSavedModel: ref => ref.sessionId === defaultRef.sessionId ? 'model-a' : 'model-b',
    onModelSelected: (ref, model) => { selections.push({ ref, model }); },
  });
  try {
    const a = await manager.ensure(defaultRef, false);
    const b = await manager.ensure(refB, false);
    assert.equal((a as any).opts.getSavedModel(), 'model-a');
    assert.equal((b as any).opts.getSavedModel(), 'model-b');
    (a as any).opts.onModelSelected('new-a');
    (b as any).opts.onModelSelected('new-b');
    assert.deepEqual(selections, [{ ref: defaultRef, model: 'new-a' }, { ref: refB, model: 'new-b' }]);
  } finally { await manager.dispose(); }
});
