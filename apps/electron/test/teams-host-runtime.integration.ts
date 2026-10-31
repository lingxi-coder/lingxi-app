/** Explicit demo-only integration fixture. Never part of the ordinary unit suite. */
import assert from 'node:assert/strict';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { execFileSync, spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { createRequire } from 'node:module';
import { once } from 'node:events';
import { createHash } from 'node:crypto';
import { createReadStream, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, statfsSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { AccountService } from '../src/main/account';
import { BotStore } from '../src/main/bots/store';
import { BotContent } from '../src/main/bots/content';
import { TeamsService } from '../src/main/teams/service';
import { TeamConnector } from '../src/main/teams/connector';
import { TeamPrivateStore } from '../src/main/teams/privateStore';
import { runTeamHost, type TeamHostBootstrap } from '../src/main/teams/process';
import { TeamOwnerApprovalBroker } from '../src/main/remoteControl/teamApprovals';
const { verifyTeamIdentifier } = await import(new URL('../scripts/credential-broker.mjs', import.meta.url).href);

const productRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const backendRequire = createRequire(new URL('../../firebase/package.json', import.meta.url));
const { initializeApp, deleteApp } = backendRequire('firebase-admin/app');
const { getAuth } = backendRequire('firebase-admin/auth');
const { getFirestore } = backendRequire('firebase-admin/firestore');
const { TeamsStore, createTeamsHandler } = await import(new URL('../../firebase/src/teams.mjs', import.meta.url).href);
const { triggerIdentity, botOutputIdentity } = await import(new URL('../../firebase/src/team-bots.mjs', import.meta.url).href);
const projectId = 'demo-lingxi-teams-t0';
assert.equal(process.platform, 'darwin', 'this actual sandboxed worker fixture is macOS only');
assert.equal(process.env.FIRESTORE_EMULATOR_HOST, '127.0.0.1:8085');
assert.equal(process.env.FIREBASE_AUTH_EMULATOR_HOST, '127.0.0.1:9098');
const disk = statfsSync(productRoot); assert.ok(disk.bavail * disk.bsize >= 2 * 1024 ** 3, '2 GiB floor; no cleanup is performed');
const binaryMode = process.env.TEAM_T3_PACKAGED_APP ? 'signedpackage' : 'debug';
let packagedApp: string | undefined;
let binaries: { host: string; worker: string };
if (process.env.TEAM_T3_PACKAGED_APP) {
  const allowed = realpathSync('/Users/luolingfeng/lingxi/.worktrees/teams/output/teams-t3-macos');
  packagedApp = realpathSync(process.env.TEAM_T3_PACKAGED_APP);
  assert.ok(packagedApp.startsWith(`${allowed}${sep}`), 'packaged app must be inside the owned Teams macOS output root');
  assert.equal(basename(packagedApp), 'LingXi Code.app'); assert.ok(lstatSync(packagedApp).isDirectory());
  const binary = (name: 'host' | 'worker'): string => {
    const candidate = join(packagedApp!, 'Contents/Resources/bin', `harness-team-${name}`);
    assert.equal(realpathSync(candidate), candidate, 'packaged Team binaries cannot escape or alias their fixed resource paths');
    execFileSync('/usr/bin/codesign', ['--verify', '--strict', candidate], { stdio: 'pipe' });
    verifyTeamIdentifier(candidate, 'AZ4AX7J833');
    return candidate;
  };
  binaries = { host: binary('host'), worker: binary('worker') };
} else {
  const companion = realpathSync(resolve(productRoot, '../harness-runtime'));
  binaries = { host: realpathSync(join(companion, 'target/debug/harness-team-host')), worker: realpathSync(join(companion, 'target/debug/harness-team-worker')) };
}
for (const binary of Object.values(binaries)) assert.ok(lstatSync(binary).isFile() && (lstatSync(binary).mode & 0o111) !== 0);
async function binaryHash(path: string): Promise<string> {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}
const binarySha256 = { host: await binaryHash(binaries.host), worker: await binaryHash(binaries.worker) };
const runId = `${Date.now().toString(36)}-${process.pid}`;
const outputRoot = join(productRoot, 'output/teams-t3-runtime');
mkdirSync(outputRoot, { recursive: true, mode: 0o700 });
const output = join(outputRoot, runId); mkdirSync(output, { mode: 0o700 });
const userData = join(output, 'user-data'), home = join(output, 'ordinary-home'), configHome = join(home, '.lingxi');
mkdirSync(userData, { mode: 0o700 }); mkdirSync(home, { mode: 0o700 }); mkdirSync(configHome, { mode: 0o700 });
writeFileSync(join(configHome, 'owner-secret.txt'), 'OWNER_DATA_MUST_NOT_BE_COPIED', { mode: 0o600 });
const teamId = `t3runtime-${runId}`, scope = { teamId, generation: 1 };
const base = 'http://127.0.0.1:18783', endpoint = `${base}/v1/teams/messages`;
const ownerUid = 'fixture-owner', desktopUid = 'desktop-human';
const humans = [desktopUid, 'ios-human', 'android-human', 'web-human', 'web-observer'];
const allowedUids = new Set([ownerUid, ...humans]);
const MODEL = 'claude-sonnet-4-20250514', QUESTION = 'Which acceptance result should I use?';
const MODEL_KEY = 'synthetic-selected-anthropic-key';
const app = initializeApp({ projectId }, `teams-main-runtime-${runId}`), auth = getAuth(app), db = getFirestore(app);
const teams = new TeamsStore(db);
const handler = createTeamsHandler({ store: teams, verifyToken: (token: string) => auth.verifyIdToken(token, true),
  createCustomToken: (uid: string) => auth.createCustomToken(uid), origins: ['http://127.0.0.1:18784'] });
const credentials = new Map<string, { idToken: string; refreshToken: string }>();
const actions: Array<{ action?: string; conversationId?: string; triggerId?: string; actualStatus: number; sentStatus: number; execute?: boolean; resultDigest?: string }> = [];
const hostLaunches: Array<{ mode: string; triggerId: string; args: string[] }> = [];
const hostOutcomes: Array<{ triggerId: string; status?: string; code?: string }> = [];
const modelRequests: Array<{ plan: string; turn: number; hasHumanAnswer: boolean }> = [];
const approvals: Array<{ triggerId: string; toolName: string }> = [];
const children = new Set<ChildProcessWithoutNullStreams>();
interface ModelPlan { kind: 'desktop' | 'offline' | 'human'; turn: number; text: string }
const plans = new Map<string, ModelPlan>();
const nativeSeeds = new Map<string, Promise<{ conversationId: string; triggerId: string; questionId: string }>>();
let service: TeamsService | undefined, connector: TeamConnector | undefined, account: AccountService | undefined;
let botKey = '', botId = '', offlineBotKey = '', failCommittedFinish = true, desktopTriggerId = '', failed = false;
let phase = 'initializing', ready = false, lastDiscoveryError: string | undefined;
let discoveryStarted = 0, discoveryCompleted = 0;
const discoveryTrace: Record<string, unknown> = {};
const approvalBroker = new TeamOwnerApprovalBroker({ botRemoteEnabled: () => true });
let stop!: () => void;
const stopped = new Promise<void>(resolveStop => { stop = resolveStop; });
process.once('SIGTERM', stop); process.once('SIGINT', stop);

function safeCode(error: unknown): string {
  const candidate = error && typeof error === 'object' && 'code' in error ? String(error.code)
    : error instanceof Error ? error.message : '';
  return /^[A-Za-z][A-Za-z0-9_/-]{0,79}$/u.test(candidate) ? candidate : error instanceof assert.AssertionError ? 'ASSERTION_FAILED' : 'UNEXPECTED_ERROR';
}
async function diagnostics(): Promise<Record<string, unknown>> {
  let pendingCount: number | string = 0, hostStatus: string | undefined;
  try { pendingCount = (await new TeamPrivateStore(userData).pending(ownerUid)).length; } catch (error) { pendingCount = safeCode(error); }
  try { if (desktopTriggerId) hostStatus = connector?.runStatus(ownerUid, teamId, 'desktop-runtime-thread', desktopTriggerId); } catch { hostStatus = 'invalid_session'; }
  const actionCounts: Record<string, number> = {};
  for (const action of actions) {
    const label = `${action.action ?? 'GET'}:${action.actualStatus}/${action.sentStatus}`;
    actionCounts[label] = (actionCounts[label] ?? 0) + 1;
  }
  return { fixture: 'teams-main-runtime', binaryMode, phase, ready, failed, discoveryStarted, discoveryCompleted, lastDiscoveryError, discoveryTrace,
    accountUid: account?.snapshot().user?.uid, accountEpoch: account?.sessionEpoch(), context: service?.hostContext(),
    actionCounts, lastActions: actions.slice(-12).map(action => ({ action: action.action ?? 'GET', actualStatus: action.actualStatus, sentStatus: action.sentStatus })),
    hostOutcomes: hostOutcomes.slice(-12), hostLaunchCount: hostLaunches.length, activeChildren: children.size,
    ownerApprovalCount: approvals.length, modelRequestCount: modelRequests.length, pendingCount, hostStatus };
}
async function progress(label: string): Promise<void> {
  phase = label;
  const value = await diagnostics(); writeFileSync(join(output, 'progress.json'), JSON.stringify(value, null, 2), { mode: 0o600 });
  console.log(JSON.stringify(value));
}
const progressTimer = setInterval(() => { void progress(phase).catch(() => { failed = true; }); }, 5000); progressTimer.unref();

async function body(request: IncomingMessage, limit = 4 * 1024 * 1024): Promise<string> {
  const chunks: Buffer[] = []; let bytes = 0;
  for await (const chunk of request) { const value = Buffer.from(chunk); bytes += value.length; assert.ok(bytes <= limit); chunks.push(value); }
  return Buffer.concat(chunks).toString('utf8');
}
function json(response: ServerResponse, value: unknown, status = 200): void {
  response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }); response.end(JSON.stringify(value));
}
async function waitFor<T>(label: string, predicate: () => Promise<T | undefined | false> | T | undefined | false, timeoutMs = 90_000, ignoreFailure = false): Promise<T> {
  await progress(label);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (failed && !ignoreFailure) { await progress(`failed: ${label}`); throw new Error(`fixture failed while waiting for ${label}`); }
    const value = await predicate(); if (value !== undefined && value !== false) return value as T;
    await new Promise(resolveWait => setTimeout(resolveWait, 200));
  }
  await progress(`timeout: ${label}`); throw new Error(`fixture timeout: ${label}`);
}
async function token(uid: string): Promise<{ idToken: string; refreshToken: string }> {
  const existing = credentials.get(uid); if (existing) return existing;
  const customToken = await auth.createCustomToken(uid);
  const response = await fetch('http://127.0.0.1:9098/identitytoolkit.googleapis.com/v1/accounts:signInWithCustomToken?key=synthetic-emulator-key', {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ token: customToken, returnSecureToken: true }),
  });
  assert.equal(response.status, 200); const value = await response.json() as { idToken: string; refreshToken: string };
  assert.ok(value.idToken && value.refreshToken); credentials.set(uid, value); return value;
}
async function post(uid: string, command: Record<string, unknown>, path = '/v1/teams/messages'): Promise<{ status: number; value: any }> {
  const credential = await token(uid);
  const response = await fetch(`${base}${path}`, { method: 'POST', headers: { Authorization: `Bearer ${credential.idToken}`, 'Content-Type': 'application/json' }, body: JSON.stringify(command) });
  return { status: response.status, value: await response.json() };
}
async function accepted(uid: string, command: Record<string, unknown>, path?: string): Promise<any> {
  const result = await post(uid, command, path); assert.equal(result.status, 200, `${command.action}: ${JSON.stringify(result.value)}`); return result.value;
}
function sse(block: Record<string, unknown>, reason: 'tool_use' | 'end_turn'): string {
  let delta: unknown;
  if (block.type === 'tool_use') { delta = { type: 'input_json_delta', partial_json: JSON.stringify(block.input) }; block = { ...block, input: {} }; }
  const frames: any[] = [
    { type: 'message_start', message: { id: 'msg_team_main_fixture', type: 'message', role: 'assistant', model: MODEL, content: [], usage: { input_tokens: 1, output_tokens: 0 } } },
    { type: 'content_block_start', index: 0, content_block: block },
    ...(delta ? [{ type: 'content_block_delta', index: 0, delta }] : []),
    { type: 'content_block_stop', index: 0 },
    { type: 'message_delta', delta: { stop_reason: reason, stop_sequence: null }, usage: { output_tokens: 3 } }, { type: 'message_stop' },
  ];
  return frames.map(frame => `event: ${frame.type}\ndata: ${JSON.stringify(frame)}\n\n`).join('');
}
function questionBlock(id: string): Record<string, unknown> {
  return { type: 'tool_use', id, name: 'AskUserQuestion', input: { questions: [{ question: QUESTION, header: 'Acceptance',
    options: [{ label: 'Current', description: 'Use the current acceptance result' }, { label: 'Archived', description: 'Use the saved acceptance result' }], multiSelect: false }] } };
}
function userText(value: any): string {
  const first = value.messages?.find((message: any) => message.role === 'user');
  if (typeof first?.content === 'string') return first.content;
  return (first?.content ?? []).filter((part: any) => part.type === 'text').map((part: any) => part.text).join('\n');
}
function registerSourcePlan(registry: Map<string, ModelPlan>, source: { teamId: string; conversationId: string; messageId: string }, text: string, kind?: ModelPlan['kind']): ModelPlan {
  assert.ok(typeof text === 'string' && text.trim(), 'accepted canonical source text must be nonempty');
  const identity = JSON.stringify([source.teamId, source.conversationId, source.messageId]);
  const existing = registry.get(identity);
  if (existing) {
    assert.equal(existing.text, text, 'an accepted source identity cannot change its original text');
    if (kind !== undefined) assert.equal(existing.kind, kind, 'a source plan cannot change its scenario');
    return existing;
  }
  const plan: ModelPlan = { kind: kind ?? 'human', turn: 0, text };
  registry.set(identity, plan);
  return plan;
}
function selectPlan(rootText: string, candidates: Iterable<ModelPlan>): ModelPlan | undefined {
  return [...candidates].find(candidate => rootText.includes(candidate.text));
}
function assertPlanIdentity(): void {
  const registry = new Map<string, ModelPlan>();
  const source = { teamId: 'fixture-team', conversationId: 'fixture-conversation', messageId: 'fixture-message' };
  const text = 'Dynamic request\n第二行 · résumé 👩🏽‍💻';
  const plan = registerSourcePlan(registry, source, text);
  const root = { role: 'user', content: [{ type: 'text', text: `Initial runtime context\n${text}\nInitial suffix` }] };
  assert.equal(selectPlan(userText({ messages: [root] }), registry.values()), plan);
  plan.turn++;
  const repeated = registerSourcePlan(registry, { ...source }, text);
  assert.equal(repeated, plan, 'an exact source retry must reuse its plan');
  assert.equal(repeated.turn, 1, 'duplicate registration must not reset model turns');
  assert.equal(registry.size, 1);
  assert.throws(() => registerSourcePlan(registry, { ...source, messageId: 'empty-message' }, ' \n\t'), /nonempty/);
  assert.throws(() => registerSourcePlan(registry, source, `${text}\nChanged`), /original text/);
  const changedRoot = { role: 'user', content: [{ type: 'text', text: `Changed runtime context\n${text}\nChanged suffix` }] };
  const followup = { messages: [changedRoot, { role: 'assistant', content: [{ type: 'tool_use', name: 'AskUserQuestion' }] },
    { role: 'user', content: [{ type: 'tool_result', content: 'Current' }] }] };
  assert.equal(selectPlan(userText(followup), registry.values()), plan, 'changed wrappers around the same multiline Unicode source must retain its plan');
  assert.equal(plan.turn, 1);
  const unrelated = { messages: [{ role: 'user', content: 'Another root request' },
    { role: 'user', content: [{ type: 'tool_result', content: plan.text }] }] };
  assert.equal(selectPlan(userText(unrelated), [plan]), undefined, 'tool results cannot select a different root plan');
  const preset: ModelPlan = { kind: 'desktop', turn: 0, text: 'T3 preset request' };
  assert.equal(selectPlan('Request wrapper\nT3 preset request\nEnd wrapper', [preset]), preset);
  const offlineSource = { ...source, messageId: 'offline-message' };
  const offline = registerSourcePlan(registry, offlineSource, 'Queued source', 'offline');
  offline.turn++;
  assert.equal(registerSourcePlan(registry, offlineSource, 'Queued source'), offline);
  assert.equal(offline.kind, 'offline', 'canonical HTTP registration must preserve preset scenarios');
  assert.equal(offline.turn, 1);
}
assertPlanIdentity();
const modelServer = createServer(async (request, response) => {
  try {
    assert.equal(request.method, 'POST'); assert.equal(request.url?.split('?')[0], '/v1/messages');
    assert.equal(request.headers['x-api-key'], MODEL_KEY);
    assert.ok(!JSON.stringify(request.headers).includes('Bearer'), 'Firebase tokens must not reach the model');
    const value = JSON.parse(await body(request)), text = userText(value);
    assert.ok(text.trim(), 'actual authenticated root human text reaches the model');
    const plan = selectPlan(text, plans.values());
    assert.ok(plan, 'actual authenticated root human text must match a registered canonical source');
    const turn = plan.turn++, max = plan.kind === 'desktop' ? 3 : plan.kind === 'offline' ? 1 : 2;
    assert.ok(turn < max, 'a completed original input must never execute again');
    const serialized = JSON.stringify(value.messages);
    const toolResults = JSON.stringify(value.messages.flatMap((message: any) => Array.isArray(message.content)
      ? message.content.filter((part: any) => part.type === 'tool_result') : []));
    const hasHumanAnswer = toolResults.includes(QUESTION) && (plan.kind !== 'desktop' || toolResults.includes('Current'));
    modelRequests.push({ plan: plan.kind, turn, hasHumanAnswer });
    let block: Record<string, unknown>, reason: 'tool_use' | 'end_turn';
    if (plan.kind === 'desktop' && turn === 0) {
      block = { type: 'tool_use', id: 'tool_fixture_bash', name: 'Bash', input: { command: "printf 'once\\n' >> team-acceptance.txt" } }; reason = 'tool_use';
    } else if ((plan.kind === 'desktop' && turn === 1) || (plan.kind === 'human' && turn === 0)) {
      block = questionBlock(`tool_question_${modelRequests.length}`); reason = 'tool_use';
    } else {
      if (plan.kind !== 'offline') {
        assert.ok(serialized.includes(QUESTION), 'the human question remains in this invocation');
        assert.ok(hasHumanAnswer, 'the exact original-human answer reaches the actual model followup');
      }
      block = { type: 'text', text: plan.kind === 'offline' ? 'Offline request completed once.' : 'Current acceptance result completed once.' }; reason = 'end_turn';
    }
    const data = sse(block, reason); response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Content-Length': Buffer.byteLength(data) }); response.end(data);
  } catch (error) {
    discoveryTrace.modelFailure = safeCode(error); await progress('synthetic model failure');
    failed = true; json(response, { error: { type: 'invalid_request_error', message: 'synthetic fixture model assertion failed' } }, 500);
  }
});

async function seed(uid: string): Promise<{ conversationId: string; triggerId: string; questionId: string }> {
  if (uid === 'web-observer') return seed('web-human');
  const old = nativeSeeds.get(uid); if (old) return old;
  const operation = (async () => {
    assert.ok(['ios-human', 'android-human', 'web-human'].includes(uid));
    const conversationId = `${uid}-live-thread`, title = uid === 'web-human' ? 'Web live Bot question' : uid === 'ios-human' ? 'iOS live Bot question' : 'Android live Bot question';
    const thread = { ...scope, conversationId };
    await accepted(uid, { action: 'create', ...thread, kind: uid === 'web-human' ? 'group' : 'bot_dm', title,
      participantUids: uid === 'web-human' ? [uid, 'web-observer'] : [uid], botKeys: [botKey] });
    const text = `T3 native original request ${uid} ${runId}`;
    const messageId = `${uid}-seed-message`, triggerId = triggerIdentity(messageId, botKey);
    registerSourcePlan(plans, { ...thread, messageId }, text, 'human');
    await accepted(uid, { action: 'mutate', ...thread, messageId, changeId: `${uid}-seed-change`, operation: 'create', text, ...(uid === 'web-human' ? { targets: [botKey] } : {}) });
    const waiting = await waitFor<any>(`${uid} real worker question`, async () => {
      const state = await accepted(uid, { action: 'botState', ...thread, triggerId }); return state.questionId ? state : undefined;
    });
    return { conversationId, triggerId, questionId: waiting.questionId };
  })();
  nativeSeeds.set(uid, operation); return operation;
}
async function fixtureState(uid: string): Promise<Record<string, unknown>> {
  const page = await accepted(uid, { action: 'list', ...scope }), conversations = [];
  let triggerCount = 0;
  for (const conversation of page.conversations) {
    const ref = db.doc(`teams/${teamId}/conversations/${conversation.id}`), triggers = await ref.collection('triggers').get();
    triggerCount += triggers.size;
    conversations.push({ id: conversation.id, title: conversation.title, triggerCount: triggers.size, messageCount: (await ref.collection('messages').get()).size });
  }
  return { projectId, teamId, botKey, offlineBotKey, conversationId: uid === 'web-observer' ? 'web-human-live-thread' : `${uid}-live-thread`,
    triggerCount, modelRequestCount: modelRequests.length, ownerApprovalCount: approvals.length, conversations, noProcessArguments: hostLaunches.every(launch => launch.args.length === 0) };
}
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? '/', base);
    if (request.method === 'GET' && url.pathname.startsWith('/_acceptance/')) {
      const uid = url.searchParams.get('uid') ?? '';
      if (!allowedUids.has(uid)) { json(response, { error: { code: 'INVALID_FIXTURE_UID' } }, 400); return; }
      if (url.pathname === '/_acceptance/diagnostics') { json(response, await diagnostics()); return; }
      if (url.pathname === '/_acceptance/bootstrap') {
        const seeded = ['ios-human', 'android-human', 'web-human', 'web-observer'].includes(uid) ? await seed(uid) : undefined;
        json(response, { customToken: await auth.createCustomToken(uid), detail: await teams.detail(uid, teamId), baseURL: base,
          localMessagesURL: endpoint, projectId, ...(seeded ?? {}), botKey, offlineBotKey }); return;
      }
      if (url.pathname === '/_acceptance/state') { json(response, await fixtureState(uid)); return; }
    }
    const raw = await body(request, 65536), command = raw ? JSON.parse(raw) : {};
    const input = new Request(`${base}${request.url}`, { method: request.method, headers: request.headers as HeadersInit,
      ...(request.method === 'POST' ? { body: raw } : {}) });
    const result = await handler(input);
    if (result.ok && request.method === 'POST' && url.pathname === '/v1/teams/messages'
      && command.action === 'mutate' && command.operation === 'create') {
      // Register accepted source text before exposing its ACK. Model envelopes
      // may change across followups; message identity and consumed turns do not.
      registerSourcePlan(plans, command, command.text);
    }
    const resultText = await result.text(), resultValue = JSON.parse(resultText || '{}');
    let status = result.status;
    if (command.action === 'finishBot' && command.triggerId === desktopTriggerId && status === 200 && failCommittedFinish) {
      failCommittedFinish = false; status = 503; // Commit succeeded; lose its ACK, retaining the exact private result.
    }
    actions.push({ action: command.action, conversationId: command.conversationId, triggerId: command.triggerId,
      actualStatus: result.status, sentStatus: status, ...(typeof resultValue.execute === 'boolean' ? { execute: resultValue.execute } : {}),
      ...(command.action === 'finishBot' ? { resultDigest: createHash('sha256').update(JSON.stringify([command.outcome, command.text])).digest('hex') } : {}) });
    response.writeHead(status, Object.fromEntries(result.headers)); response.end(status === result.status ? resultText : JSON.stringify({ error: { code: 'SERVICE_UNAVAILABLE' } }));
  } catch (error) {
    discoveryTrace.backendFailure = safeCode(error); await progress('unexpected fixture backend failure');
    failed = true; json(response, { error: { code: 'FIXTURE_FAILURE' } }, 500);
  }
});

function files(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => entry.isDirectory() ? files(join(directory, entry.name)) : entry.isFile() ? [join(directory, entry.name)] : []);
}
async function startConnector(): Promise<void> {
  assert.ok(account);
  const store = new BotStore({ userData, home, defaultConfigHome: configHome }), privateStore = new TeamPrivateStore(userData);
  service = new TeamsService(account, { projectId, apiKey: 'synthetic-emulator-key', appId: 'teams-main-runtime' }, base,
    { host: '127.0.0.1', authPort: 9098, firestorePort: 8085 });
  const actualList = service.list.bind(service), actualHostMessages = service.hostMessages.bind(service);
  service.list = async uid => {
    try {
      const value = await actualList(uid);
      discoveryTrace.teams = { count: value.teams.length, current: value.teams.find(team => team.id === teamId) ?? null };
      return value;
    } catch (error) { discoveryTrace.listError = safeCode(error); throw error; }
  };
  service.hostMessages = async <T>(context: Parameters<TeamsService['hostMessages']>[0], command: Record<string, unknown>): Promise<T> => {
    try {
      const value = await actualHostMessages<T>(context, command);
      if (command.action === 'bots') {
        const cards = (value as { bots: Array<Record<string, unknown>> }).bots;
        const local = store.get(botId).teams?.find(binding => binding.ownerUid === ownerUid && binding.teamId === teamId);
        const card = cards.find(candidate => candidate.botId === botId);
        discoveryTrace.cards = { count: cards.length, localBinding: Boolean(local), card: Boolean(card),
          ...(local && card ? { revisionMatches: local.revision === card.revision, keyMatches: local.botKey === card.key,
            hostMatches: privateStore.hostId === card.hostId, enabled: card.enabled, policyMatches: local.use === card.use && JSON.stringify(local.uids) === JSON.stringify(card.uids) } : {}) };
      }
      discoveryTrace.lastHostAction = command.action;
      return value;
    } catch (error) { discoveryTrace.hostActionError = { action: command.action, code: safeCode(error) }; throw error; }
  };
  connector = new TeamConnector({ service, store, content: new BotContent(store), privateStore, retryMs: 250,
    binaries: () => binaries, defaultModel: () => MODEL,
    modelRoute: async () => ({ route: { model_provider: 'anthropic', model_protocol: 'anthropic_messages', model_id: MODEL,
      model_endpoint: 'https://api.anthropic.com', credential_id: 'anthropic-api-key',
      model_profile: { provider_id: 'anthropic', profile_name: 'anthropic', base_url: 'https://api.anthropic.com',
        protocol: 'anthropic_messages', auth: 'api_key', credential: { source: 'host_managed', key: 'team-selected-model' },
        models: [{ display_model: MODEL, request_model: MODEL, billing_model: MODEL,
          capability_support: { streaming: 'supported', tools: 'supported' } }], extra: {} } }, assertCurrent: async () => {} }),
    modelCredential: async credentialId => { assert.equal(credentialId, 'anthropic-api-key'); return MODEL_KEY; },
    permission: (input, signal) => approvalBroker.request({ ...input, showLocal: async () => {
      assert.equal(input.target.ownerUid, ownerUid); assert.equal(input.toolName, 'Bash');
      assert.equal(input.target.triggerId, desktopTriggerId); approvals.push({ triggerId: input.target.triggerId, toolName: input.toolName });
      assert.equal(approvals.length, 1, 'owner allows this physical Bash exactly once'); return true;
    } }, signal),
    run: async input => {
      assert.equal(input.bootstrap.endpoint, 'https://lingxios.net/v1/teams/messages');
      assert.ok(!input.bootstrap.snapshot || input.bootstrap.snapshot.model_endpoint === 'https://api.anthropic.com');
      // The only URL replacement is this injected fixture wrapper. No production
      // environment switch, account setting, renderer or deep link can select it.
      const modelAddress = modelServer.address(); assert.ok(modelAddress && typeof modelAddress !== 'string');
      const bootstrap = { ...input.bootstrap, endpoint,
        ...(input.bootstrap.snapshot ? { snapshot: { ...input.bootstrap.snapshot, model_endpoint: `http://127.0.0.1:${modelAddress.port}`,
          model_profile: { ...input.bootstrap.snapshot.model_profile, base_url: `http://127.0.0.1:${modelAddress.port}` } } } : {}) } as unknown as TeamHostBootstrap;
      try {
        const result = await runTeamHost({ ...input, bootstrap, spawnProcess: (executable, args, options) => {
          assert.deepEqual(args, []); assert.equal(executable, binaries.host);
          assert.deepEqual(Object.keys(options!.env!).sort(), ['CFFIXED_USER_HOME', 'HOME', 'LANG', 'LC_ALL', 'LINGXI_CONFIG_DIR', 'PATH', 'TMPDIR', '__CF_USER_TEXT_ENCODING'].sort());
          hostLaunches.push({ mode: bootstrap.mode, triggerId: bootstrap.target.triggerId, args: [...args] });
          const child = spawn(executable, args, options) as ChildProcessWithoutNullStreams;
          children.add(child); child.once('close', () => children.delete(child)); return child;
        } });
        hostOutcomes.push({ triggerId: bootstrap.target.triggerId, status: result.status }); return result;
      } catch (error) {
        const code = error && typeof error === 'object' && 'code' in error ? String(error.code) : 'HOST_FAILED';
        hostOutcomes.push({ triggerId: bootstrap.target.triggerId, code });
        if (bootstrap.target.triggerId === desktopTriggerId && code === 'RESULT_PENDING') {
          // Stop before the next poll, proving the persisted index survives a
          // process lifecycle even though the backend queue already advanced.
          service?.close(); service = undefined; connector = undefined;
        }
        throw error;
      }
    },
  });
  // Fixture-only instrumentation around the actual discovery method. It does
  // not change the return, retries, bindings or caught production behaviour.
  const instrumented = connector as unknown as { discover(): Promise<void> };
  const actualDiscovery = instrumented.discover.bind(connector);
  instrumented.discover = async () => {
    discoveryStarted++;
    try { await actualDiscovery(); discoveryCompleted++; lastDiscoveryError = undefined; }
    catch (error) { lastDiscoveryError = safeCode(error); throw error; }
  };
  service.attachHost(connector); await connector.idle();
}
async function stopConnector(): Promise<void> {
  service?.close(); service = undefined; connector = undefined;
  await waitFor('host processes closed', () => children.size === 0 ? true : undefined, 10_000, true);
}

try {
  modelServer.listen(0, '127.0.0.1'); await once(modelServer, 'listening');
  server.listen(18783, '127.0.0.1'); await once(server, 'listening');
  for (const uid of allowedUids) {
    try { await auth.createUser({ uid, displayName: uid }); } catch (error) { if ((error as { code?: string }).code !== 'auth/uid-already-exists') throw error; }
  }
  const ownerCredential = await token(ownerUid);
  let storedAccount = JSON.stringify({ projectId, refreshToken: ownerCredential.refreshToken, user: { uid: ownerUid, email: null, displayName: 'Fixture owner', emailVerified: false } });
  account = new AccountService({ projectId, apiKey: 'synthetic-emulator-key', appId: 'teams-main-runtime', authDomain: `${projectId}.firebaseapp.com` },
    { persistent: false, read: async () => storedAccount, write: async value => { storedAccount = value; }, remove: async () => { storedAccount = ''; } },
    async () => { throw new Error('no browser/account login in fixture'); }, (async (url, init) => {
      const source = new URL(String(url)); assert.ok(['securetoken.googleapis.com', 'identitytoolkit.googleapis.com'].includes(source.hostname));
      return fetch(`http://127.0.0.1:9098/${source.hostname}${source.pathname}${source.search}`, init);
    }) as typeof fetch);
  await account.execute({ type: 'status' }); assert.equal(account.snapshot().user?.uid, ownerUid);
  assert.ok(await account.getIdToken());
  await accepted(ownerUid, { action: 'create', teamId, name: 'Teams T3 live acceptance' }, '/v1/teams');
  const invitation = await accepted(ownerUid, { action: 'invite', teamId, requestId: `invite-${runId}`, role: 'member', maxUses: humans.length, ttlSeconds: 3600 }, '/v1/teams');
  for (const uid of humans) await accepted(uid, { action: 'join', code: invitation.code, requestId: `join-${uid}-${runId}` }, '/v1/teams');
  const store = new BotStore({ userData, home, defaultConfigHome: configHome });
  const bot = store.create({ name: 'T3 acceptance Bot', model: `anthropic/${MODEL}`, persona: 'Frozen fixture persona. Use only the Team request and private workspace.' }); botId = bot.bot_id;
  await startConnector();
  const bound = await service!.bindLocalBot(ownerUid, { ...scope, botId, use: 'all', uids: [], expectedRevision: 0 }); botKey = bound.botKey;
  const offline = await accepted(ownerUid, { action: 'bindBot', ...scope, hostId: 'o'.repeat(22), botId: 'f'.repeat(22), expectedRevision: 0,
    name: 'T3 offline acceptance Bot', description: 'Deliberately unstarted synthetic host', use: 'all', uids: [] }); offlineBotKey = offline.botKey;
  await waitFor('SDK listeners warm', () => actions.some(action => action.action === 'heartbeatBot') ? true : undefined);
  const conversationId = 'desktop-runtime-thread', thread = { ...scope, conversationId }, messageId = 'desktop-runtime-message';
  await accepted(desktopUid, { action: 'create', ...thread, kind: 'bot_dm', title: 'Desktop host live acceptance', participantUids: [desktopUid], botKeys: [botKey] });
  const text = `T3 desktop original request ${runId}`;
  registerSourcePlan(plans, { ...thread, messageId }, text, 'desktop'); desktopTriggerId = triggerIdentity(messageId, botKey);
  await accepted(desktopUid, { action: 'mutate', ...thread, messageId, changeId: 'desktop-runtime-change', operation: 'create', text });
  const waiting = await waitFor<any>('actual original-human question', async () => {
    const state = await accepted(desktopUid, { action: 'botState', ...thread, triggerId: desktopTriggerId }); return state.questionId ? state : undefined;
  });
  assert.equal(waiting.state, 'running'); assert.equal(waiting.answered, false); assert.equal(waiting.origin.uid, desktopUid);
  assert.ok(!JSON.stringify(waiting).includes('command'), 'public waiting state must not expose tool arguments');
  assert.equal(approvals.length, 1);
  const answer = { action: 'botAnswer', ...thread, triggerId: desktopTriggerId, questionId: waiting.questionId, answerId: 'desktop-answer01', answers: { [QUESTION]: 'Current' } };
  assert.equal((await post(ownerUid, answer)).status, 403, 'Bot owner is not the original-human answer capability');
  await accepted(desktopUid, answer); await accepted(desktopUid, answer);
  await waitFor('finish ACK loss recorded', () => hostOutcomes.some(value => value.triggerId === desktopTriggerId && value.code === 'RESULT_PENDING') ? true : undefined);
  await stopConnector();
  const stableHostId = new TeamPrivateStore(userData).hostId;
  assert.ok((await new TeamPrivateStore(userData).pending(ownerUid)).some(value => value.target.triggerId === desktopTriggerId));
  await startConnector();
  await waitFor('durable exact-result retry ACK after restart', () => hostOutcomes.some(value => value.triggerId === desktopTriggerId && value.status === 'settled') ? true : undefined);
  const ref = db.doc(`teams/${teamId}/conversations/${conversationId}`), run = (await ref.collection('botRuns').doc(desktopTriggerId).get()).data();
  assert.equal(run.state, 'completed'); assert.equal(run.replyId, botOutputIdentity(desktopTriggerId, 'reply'));
  assert.equal((await ref.collection('messages').where('triggerId', '==', desktopTriggerId).where('kind', '==', 'text').get()).size, 1);
  const finish = actions.filter(action => action.action === 'finishBot' && action.triggerId === desktopTriggerId);
  assert.ok(finish.length >= 2); assert.equal(new Set(finish.map(value => value.resultDigest)).size, 1);
  assert.equal(actions.filter(action => action.action === 'startBot' && action.triggerId === desktopTriggerId && action.execute === true).length, 1);
  assert.equal(modelRequests.filter(value => value.plan === 'desktop').length, 3);
  const evidence = files((await new TeamPrivateStore(userData).owner(ownerUid)).runtime).filter(path => path.endsWith('/workspace/team-acceptance.txt'));
  assert.equal(evidence.length, 1); assert.equal(readFileSync(evidence[0]!, 'utf8'), 'once\n');
  assert.ok(!files((await new TeamPrivateStore(userData).owner(ownerUid)).runtime).some(path => path.endsWith('/owner-secret.txt')));
  await stopConnector();
  const offlineMessageId = 'offline-runtime-message', offlineTrigger = triggerIdentity(offlineMessageId, botKey), offlineText = `T3 offline queued request ${runId}`;
  registerSourcePlan(plans, { ...thread, messageId: offlineMessageId }, offlineText, 'offline');
  const modelCount = modelRequests.length;
  await accepted(desktopUid, { action: 'mutate', ...thread, messageId: offlineMessageId, changeId: 'offline-runtime-change', operation: 'create', text: offlineText });
  const queued = await accepted(desktopUid, { action: 'botState', ...thread, triggerId: offlineTrigger }); assert.equal(queued.state, 'queued');
  assert.equal(modelRequests.length, modelCount);
  await startConnector(); assert.equal(new TeamPrivateStore(userData).hostId, stableHostId);
  await waitFor('offline queued request completed once', async () => (await ref.collection('botRuns').doc(offlineTrigger).get()).data()?.state === 'completed' ? true : undefined);
  assert.equal(modelRequests.filter(value => value.plan === 'offline').length, 1);
  assert.equal(await binaryHash(binaries.host), binarySha256.host); assert.equal(await binaryHash(binaries.worker), binarySha256.worker);
  const report = { projectId, runId, binaryMode, binarySha256, ...(packagedApp ? { packagedApp } : {}), teamId, botKey, offlineBotKey, baseURL: base, localMessagesURL: endpoint, output,
    checks: { actualAccountService: true, realAuthFirestoreHttp: true, ownerApprovals: approvals.length, nonOriginAnswer: 403,
      exactAnswerRetries: 2, modelTurns: 4, finalDesktopReplies: 1, finishAckLostAfterCommit: true, stableRestart: true, offlineQueuedOnce: true,
      noProcessArguments: hostLaunches.every(launch => launch.args.length === 0) }, hostLaunches, hostOutcomes };
  writeFileSync(join(output, 'result.json'), JSON.stringify(report, null, 2), { mode: 0o600 });
  if (process.env.TEAM_T3_NATIVE === '1') {
    ready = true;
    writeFileSync(join(outputRoot, 'ready.json'), JSON.stringify({ ...report, ready, authPort: 9098, firestorePort: 8085, bootstrapURL: `${base}/_acceptance/bootstrap`, stateURL: `${base}/_acceptance/state` }, null, 2), { mode: 0o600 });
    await progress('ready for native acceptance');
    console.log(JSON.stringify({ fixture: 'teams-main-runtime', ready: true, readyFile: join(outputRoot, 'ready.json'), projectId, teamId, baseURL: base }));
    await stopped;
  } else console.log(JSON.stringify({ fixture: 'teams-main-runtime', passed: true, report: join(output, 'result.json'), checks: report.checks }));
} finally {
  clearInterval(progressTimer);
  await stopConnector(); approvalBroker.close(); account?.dispose();
  server.closeAllConnections(); modelServer.closeAllConnections();
  if (server.listening) await new Promise<void>(resolveClose => server.close(() => resolveClose()));
  if (modelServer.listening) await new Promise<void>(resolveClose => modelServer.close(() => resolveClose()));
  await db.terminate(); await deleteApp(app);
}
