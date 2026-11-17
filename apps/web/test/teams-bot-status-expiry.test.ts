import assert from 'node:assert/strict';
import test from 'node:test';
import { TeamsWorkspace } from '../../../packages/teams/workspace';
import { FakeTeams, botCard, memoryStorage, team } from './support/fakeTeams';
const flush = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };

test('A maintenance status expires after 90 seconds without a signal and reloads as offline', async context => {
  const start = Date.UTC(2026, 10, 16); context.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: start });
  const fake = new FakeTeams(); fake.teams = [team('team-test')];
  fake.bots = [botCard({ ready: false, availability: 'maintenance', onlineUntil: start + 600000, statusExpiresAt: start + 90000 })];
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close()); await flush();
  assert.equal(workspace.getSnapshot().bots[0]?.availability, 'maintenance');
  const before = fake.commands.filter(command => command.action === 'listSharedBots').length;
  context.mock.timers.tick(89999); await flush(); assert.equal(fake.commands.filter(command => command.action === 'listSharedBots').length, before);
  fake.bots = [botCard({ ready: false, availability: 'offline', onlineUntil: start, statusExpiresAt: undefined })];
  context.mock.timers.tick(1); await flush();
  assert.equal(fake.commands.filter(command => command.action === 'listSharedBots').length, before + 1);
  assert.equal(workspace.getSnapshot().bots[0]?.availability, 'offline');
});

test('An expired ready projection becomes unverified during its reread and stays unverified after a network failure', async context => {
  const start = Date.UTC(2026, 10, 16); context.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: start });
  const fake = new FakeTeams(); fake.teams = [team('team-test')]; fake.bots = [botCard({ ready: true, availability: 'ready', onlineUntil: start + 90000, statusExpiresAt: start + 90000 })];
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close()); await flush();
  let reject!: (error: Error) => void; const pending = new Promise<never>((_, fail) => { reject = fail; }), original = fake.messages;
  fake.messages = async command => command.action === 'listSharedBots' ? pending : original(command);
  context.mock.timers.tick(90000); await flush();
  assert.equal(workspace.getSnapshot().bots[0]?.availability, 'unverified'); assert.equal(workspace.getSnapshot().bots[0]?.ready, false);
  reject(new Error('SERVICE_UNAVAILABLE')); await flush(); assert.equal(workspace.getSnapshot().botsUnverified, true); assert.equal(workspace.getSnapshot().bots.length, 0);
});

test('A legacy ready card without statusExpiresAt retains its onlineUntil refresh deadline', async context => {
  const start = Date.UTC(2026, 10, 16); context.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: start });
  const fake = new FakeTeams(); fake.teams = [team('team-test')]; fake.bots = [botCard({ ready: true, onlineUntil: start + 90000 })];
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close()); await flush();
  const before = fake.commands.filter(command => command.action === 'listSharedBots').length;
  fake.bots = [botCard({ ready: false, onlineUntil: start + 90000 })]; context.mock.timers.tick(90000); await flush();
  assert.equal(fake.commands.filter(command => command.action === 'listSharedBots').length, before + 1); assert.equal(workspace.getSnapshot().bots[0]?.ready, false);
});

for (const code of ['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP']) test(`${code} purges internal cards/examples and cancels their old expiry without disposing the workspace`, async context => {
  const start = Date.UTC(2026, 10, 16); context.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: start });
  const fake = new FakeTeams(); fake.teams = [team('team-test')]; fake.bots = [botCard({ ready: false, availability: 'maintenance', examples: ['Previously authorized example'], statusExpiresAt: start + 90000 })];
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close()); await flush();
  assert.equal(workspace.getSnapshot().bots[0]?.examples?.[0], 'Previously authorized example'); assert.equal(workspace.getSnapshot().blockedBotIds.length, 1);
  const original = fake.messages; fake.messages = async command => { if (command.action === 'listSharedBots') throw new Error(code); return original(command); };
  context.mock.timers.tick(1000); await workspace.refreshBots(); await flush();
  assert.deepEqual(workspace.getSnapshot().bots, []); assert.deepEqual(workspace.getSnapshot().blockedBotIds, []); assert.equal(workspace.getSnapshot().botsError, code);
  const observed: ReturnType<TeamsWorkspace['getSnapshot']>[] = []; const stop = workspace.subscribe(() => observed.push(workspace.getSnapshot())); context.after(stop);
  let resolve!: (value: never) => void, reads = 0; const pending = new Promise<never>(done => { resolve = done; });
  fake.messages = async command => { if (command.action === 'listSharedBots') { reads++; return pending; } return original(command); };
  const reading = workspace.refreshBots(); await flush(); assert.equal(reads, 1);
  context.mock.timers.tick(100000); await flush();
  assert.deepEqual(workspace.getSnapshot().bots, []); assert.deepEqual(workspace.getSnapshot().blockedBotIds, []);
  assert.ok(observed.every(snapshot => snapshot.bots.length === 0), 'no intermediate snapshot restores a revoked card or its examples');
  resolve({ bots: [] } as never); await reading; await flush(); assert.equal(reads, 1, 'the rejected scope did not leave a TTL retry queued');
});

test('A transient read failure cannot resurrect discarded cards from an earlier internal cache at their old deadline', async context => {
  const start = Date.UTC(2026, 10, 16); context.mock.timers.enable({ apis: ['setTimeout', 'Date'], now: start });
  const fake = new FakeTeams(); fake.teams = [team('team-test')]; fake.bots = [botCard({ sharedBotId: 'ready-card', ready: true, availability: 'ready', examples: ['Discarded ready example'], statusExpiresAt: start + 5000 }), botCard({ sharedBotId: 'blocked-card', ready: false, availability: 'maintenance', examples: ['Retained blocked example'], statusExpiresAt: start + 90000 })];
  const workspace = new TeamsWorkspace(fake.api, 'alice', { storage: memoryStorage, environment: new EventTarget() }); context.after(() => workspace.close()); await flush();
  const original = fake.messages; fake.messages = async command => { if (command.action === 'listSharedBots') throw new Error('SERVICE_UNAVAILABLE'); return original(command); };
  await workspace.refreshBots(); assert.deepEqual(workspace.getSnapshot().bots.map(bot => bot.sharedBotId), ['blocked-card']);
  let resolve!: (value: never) => void; const pending = new Promise<never>(done => { resolve = done; }); fake.messages = async command => command.action === 'listSharedBots' ? pending : original(command);
  const reading = workspace.refreshBots(); await flush(); context.mock.timers.tick(5000); await flush();
  assert.deepEqual(workspace.getSnapshot().bots.map(bot => bot.sharedBotId), ['blocked-card']); assert.equal(JSON.stringify(workspace.getSnapshot().bots).includes('Discarded ready example'), false);
  resolve({ bots: [] } as never); await reading; await flush();
});
