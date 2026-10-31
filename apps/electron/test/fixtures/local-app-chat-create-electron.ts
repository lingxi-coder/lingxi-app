import { app, BrowserWindow, ipcMain, protocol, session } from 'electron';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { LOCAL_APP_SDK } from '../../src/preload/localAppSdk';
import { CH_LOCAL_APP_NATIVE } from '../../src/shared/localAppNative';
import { LocalAppAgentBroker } from '../../src/main/local-apps/agentBroker';
import { LocalAppHost } from '../../src/main/local-apps/host';

const root = process.env.LINGXI_TEST_USER_DATA!;
const appId = 'a11111111111111111111111111111111';
const digest = '1'.repeat(64), invocation = 'invocation-1';
app.setPath('userData', join(root, 'profile'));
protocol.registerSchemesAsPrivileged([{ scheme: 'lingxi-app', privileges: { standard: true, secure: true, supportFetchAPI: true } }]);
process.on('uncaughtException', error => { process.stderr.write(String(error.stack)); app.exit(1); });
process.on('unhandledRejection', error => { process.stderr.write(String(error)); app.exit(1); });

async function main() {
  await app.whenReady();
  const settings = { enabled: true, model: { providerId: 'test', modelId: 'test' }, tools: [], skills: [], background: false, maxTurns: 20, timeoutMs: 600_000 };
  const authorization = { settings, requestedTools: [], requestedSkills: [], releaseRevision: 1, releaseSourceDirectory: '/test', skillSpecs: [], releaseFiles: [],
    model: { providerId: 'test', modelId: 'test', label: 'Test', canSelect: true, supportsAttachments: true, supportsVision: true, supportsStructuredOutput: true } };
  const broker = new LocalAppAgentBroker({ root: join(root, 'storage'), authorize: async () => authorization, revalidate: async () => true,
    execute: async () => { throw new Error('not used'); } });
  const host = new LocalAppHost({ configHome: root, core: {} as any, preload: join(root, 'preload.cjs'), mainWindow: () => undefined, edit: async () => undefined });
  const partition = session.fromPartition(`persist:local-app-${appId}`);
  partition.protocol.handle('lingxi-app', request => {
    const path = new URL(request.url).pathname;
    return path === '/sdk.js' ? new Response(LOCAL_APP_SDK, { headers: { 'content-type': 'application/javascript' } })
      : new Response('<!doctype html><script src="/sdk.js"></script>', { headers: { 'content-type': 'text/html' } });
  });
  const window = new BrowserWindow({ show: false, webPreferences: { partition: `persist:local-app-${appId}`, preload: join(root, 'preload.cjs'), contextIsolation: true, nodeIntegration: false, sandbox: true } });
  const internals = host as any;
  internals.windows.set(appId, window);
  internals.windowRuns.set(window, invocation);
  internals.navigationEpoch.set(window.webContents.id, 1);
  internals.launchGrants.set(appId, ['agent']);
  internals.store.dataPath = () => join(root, 'data');
  internals.store.getRevision = async () => ({ name: 'Test', grants: ['agent'], releaseDigest: digest });
  internals.runtime.revision = () => 1;
  internals.runtime.owner = () => ({ appId, releaseDigest: digest });
  internals.runtime.invocationId = () => invocation;
  internals.runtime.phase = () => 'running';
  host.setAgentBroker(broker);
  let createCalls = 0, loseFirstReply = true;
  ipcMain.handle(CH_LOCAL_APP_NATIVE, async (event, input) => {
    const result = await internals.native(event, input);
    if (input.method === 'agent.chats.create') {
      createCalls++;
      if (loseFirstReply) { loseFirstReply = false; throw new Error('simulated IPC disconnect after commit'); }
    }
    return result;
  });
  await window.loadURL(`lingxi-app://${appId}/`);
  const request = { requestId: '11111111-1111-4111-8111-111111111111', title: 'Plan' };
  const createResult = await window.webContents.executeJavaScript(`localApp.agent.chats.create(${JSON.stringify(request)}).then(() => null, e => ({code:e.code,requestId:e.requestId}))`);
  assert.deepEqual(createResult, { code: 'host_unavailable', requestId: request.requestId });
  assert.equal(createCalls, 1);
  const reloaded = new Promise<void>(resolve => window.webContents.once('did-finish-load', () => resolve()));
  window.reload();
  await reloaded;
  assert.deepEqual(await window.webContents.executeJavaScript('localApp.agent.chats.pendingCreations()'), [{ ...request, state: 'pending' }]);
  assert.equal(createCalls, 1);
  assert.equal(await window.webContents.executeJavaScript(`localApp.agent.chats.creationStatus(${JSON.stringify(request)}).then(x => x.id)`), request.requestId);
  assert.deepEqual(await window.webContents.executeJavaScript('localApp.agent.chats.pendingCreations()'), [{ ...request, state: 'accepted' }]);
  let entered!: () => void, release!: () => void;
  const enteredPromise = new Promise<void>(resolve => { entered = resolve; });
  const released = new Promise<void>(resolve => { release = resolve; });
  const originalStatus = broker.chatCreationStatus.bind(broker);
  broker.chatCreationStatus = async (...args) => { const result = await originalStatus(...args); entered(); await released; return result; };
  const oldRevisionStatus = window.webContents.executeJavaScript(`localApp.agent.chats.creationStatus(${JSON.stringify(request)}).then(x => x.id, e => e.code)`);
  await enteredPromise;
  internals.runtime.revision = () => 2;
  release();
  assert.equal(await oldRevisionStatus, 'host_unavailable');
  broker.chatCreationStatus = originalStatus;
  internals.runtime.revision = () => 1;
  await window.webContents.executeJavaScript(`localApp.agent.chats.open(${JSON.stringify(request.requestId)}).then(chat => { window.fixtureChat = chat; })`);
  const cases = [
    { method: 'getChat', expression: `localApp.agent.chats.open(${JSON.stringify(request.requestId)})`, value: { chatId: request.requestId } },
    { method: 'chatHistory', expression: 'window.fixtureChat.history()', value: { messages: [] } },
    { method: 'get', expression: `localApp.agent.tasks.get('old-run')`, value: { runId: 'old-run', state: 'succeeded' } },
    { method: 'events', expression: `localApp.agent.tasks.events({runId:'old-run'})`, value: { events: [], sequence: 0 } },
    { method: 'list', expression: 'localApp.agent.tasks.list()', value: { tasks: [] } },
  ];
  for (const [index, testCase] of cases.entries()) {
    let requestEntered!: () => void, releaseRead!: () => void;
    const started = new Promise<void>(resolve => { requestEntered = resolve; });
    const gate = new Promise<void>(resolve => { releaseRead = resolve; });
    const original = (broker as any)[testCase.method];
    (broker as any)[testCase.method] = async () => { requestEntered(); await gate; return testCase.value; };
    const pending = window.webContents.executeJavaScript(`${testCase.expression}.then(() => 'leaked', () => 'blocked')`);
    await started;
    if (index % 2 === 0) internals.runtime.revision = () => 2;
    else internals.blockedAgentApps.add(appId);
    releaseRead();
    assert.equal(await pending, 'blocked', `${testCase.method} cannot return data after access changes`);
    (broker as any)[testCase.method] = original;
    internals.runtime.revision = () => 1;
    internals.blockedAgentApps.delete(appId);
  }
  assert.equal(await window.webContents.executeJavaScript(`localApp.agent.chats.creationStatus({requestId:${JSON.stringify(request.requestId)},title:'Other'}).catch(e => e.code)`), 'chat_create_conflict');
  (broker as any).uncertainChats.add(`${appId}:${request.requestId}`);
  assert.deepEqual(await window.webContents.executeJavaScript(`localApp.agent.chats.list()`), { chats: [], hasUncertainChats: true, unavailableChats: [{ chatId: request.requestId, code: 'chat_storage_uncertain' }] });
  (broker as any).uncertainChats.delete(`${appId}:${request.requestId}`);
  await window.webContents.executeJavaScript(`localApp.agent.chats.delete(${JSON.stringify(request.requestId)})`);
  assert.equal(await window.webContents.executeJavaScript(`localApp.agent.chats.create(${JSON.stringify(request)}).catch(e => e.code)`), 'chat_create_deleted');
  assert.deepEqual(await window.webContents.executeJavaScript('localApp.agent.chats.pendingCreations()'), [{ ...request, state: 'deleted' }]);
  await window.webContents.executeJavaScript(`localApp.agent.chats.forgetCreation(${JSON.stringify(request.requestId)})`);
  assert.deepEqual(await window.webContents.executeJavaScript('localApp.agent.chats.pendingCreations()'), []);
  // Current unsafe/unavailable storage must preserve its error across host IPC
  // and isolated preload, rather than becoming an empty Chat list.
  (broker as any).unavailableApps.add(appId);
  const unavailableRequest = { requestId: '22222222-2222-4222-8222-222222222222' };
  assert.deepEqual(await window.webContents.executeJavaScript(`localApp.agent.chats.create(${JSON.stringify(unavailableRequest)}).catch(e => ({code:e.code,message:e.message}))`),
    { code: 'agent_storage_unavailable', message: 'This Local App has invalid or unsafe Agent storage. Its data has been preserved.' });
  assert.equal(await window.webContents.executeJavaScript(`localApp.agent.chats.list().catch(e => e.code)`), 'agent_storage_unavailable');
  assert.deepEqual(await window.webContents.executeJavaScript('localApp.agent.chats.pendingCreations()'), [{ ...unavailableRequest, state: 'pending' }]);
  window.destroy();
  ipcMain.removeHandler(CH_LOCAL_APP_NATIVE);
  await broker.dispose();
  process.stdout.write(JSON.stringify({ passed: true, createCalls }) + '\n');
  app.exit(0);
}
void main();
