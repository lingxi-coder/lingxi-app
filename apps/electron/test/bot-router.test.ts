import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer, request, type IncomingHttpHeaders } from 'node:http';
import type { AddressInfo } from 'node:net';

import type { RemoteObserver } from '../src/main/bridgeTypes.js';
import { BotRouter, SUPPORTED_PROTOCOLS, type RuntimeSource, type WatchedRuntime } from '../src/main/bots/router.js';
import {
  ROUTER_TOOL_NAMES,
  type Caller,
  type DelegationBackend,
  type TaskView,
} from '../src/main/bots/routerTools.js';

const BOT = 'Bot0000000000000000001';
const OTHER_BOT = 'Bot0000000000000000002';
const SESSION = '11111111-2222-4333-8444-555555555555';
const OTHER_SESSION = '99999999-2222-4333-8444-555555555555';
const TASK = 'Task000000000000000001';

function view(): TaskView {
  return { task_id: TASK, bot: OTHER_BOT, bot_name: 'Other', status: { state: 'working', timestamp: '2026-10-27T10:00:00.000Z' }, deadline: '2026-10-27T10:30:00.000Z', artifacts: [], follow_ups: 0 };
}

/** A backend that notes who called and what for, and answers as the test says. */
function fakeBackend() {
  const calls: Array<{ method: string; caller: Caller; args: unknown[] }> = [];
  const hooks: { getTask?: (waitSeconds: number, signal: AbortSignal) => Promise<TaskView> } = {};
  const backend: DelegationBackend = {
    listBots: (caller) => { calls.push({ method: 'listBots', caller, args: [] }); return [{ id: OTHER_BOT, name: 'Other', description: 'Does things', accepts_tasks_from_you: true }]; },
    delegate: async (caller, args) => { calls.push({ method: 'delegate', caller, args: [args] }); return view(); },
    getTask: async (caller, taskId, waitSeconds, signal) => { calls.push({ method: 'getTask', caller, args: [taskId, waitSeconds] }); return hooks.getTask ? hooks.getTask(waitSeconds, signal) : view(); },
    messageBot: async (caller, taskId, text) => { calls.push({ method: 'messageBot', caller, args: [taskId, text] }); return view(); },
    cancelTask: async (caller, taskId) => { calls.push({ method: 'cancelTask', caller, args: [taskId] }); return view(); },
    attachArtifact: async (caller, taskId, path) => { calls.push({ method: 'attachArtifact', caller, args: [taskId, path] }); return { task_id: taskId, artifact: { name: 'a', path: 'inbox/a', size: 1, sha256: '0'.repeat(64) } }; },
  };
  return { backend, calls, hooks };
}

async function setup(t: { after(fn: () => void | Promise<void>): void }, options: { maxRequestBytes?: number } = {}) {
  const fake = fakeBackend();
  const warnings: string[] = [];
  const router = new BotRouter({ backend: fake.backend, warn: (message) => warnings.push(message), ...options });
  t.after(() => router.dispose());
  const url = await router.start();
  const { port } = new URL(url);
  const token = router.issue(BOT, SESSION);
  return { ...fake, router, url, port, token, warnings };
}

interface Reply { status: number; headers: IncomingHttpHeaders; text: string }

function send(port: string, options: { method?: string; path?: string; headers?: Record<string, string>; body?: string | Buffer; token?: string | null } = {}): Promise<Reply> {
  return new Promise((resolve, reject) => {
    const headers: Record<string, string> = { ...(options.token === null ? {} : options.token !== undefined ? { Authorization: `Bearer ${options.token}` } : {}), ...options.headers };
    const req = request({ host: '127.0.0.1', port: Number(port), path: options.path ?? '/mcp', method: options.method ?? 'POST', headers }, (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode ?? 0, headers: res.headers, text: Buffer.concat(chunks).toString('utf8') }));
      res.on('error', reject);
    });
    req.on('error', reject);
    if (options.body !== undefined) req.write(options.body);
    req.end();
  });
}

const JSON_HEADERS = { 'Content-Type': 'application/json', Accept: 'application/json, text/event-stream' };

function rpc(port: string, token: string | null, message: unknown, headers: Record<string, string> = {}): Promise<Reply> {
  return send(port, { token, headers: { ...JSON_HEADERS, ...headers }, body: JSON.stringify(message) });
}

const call = (id: number, name: string, args: unknown) => ({ jsonrpc: '2.0', id, method: 'tools/call', params: { name, arguments: args } });

test('the router listens on the loopback address and says where, and where it was asked to be again', async (t) => {
  const s = await setup(t);
  assert.match(s.url, /^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
  assert.equal(await s.router.start(), s.url);
  assert.equal(await s.router.ready(), s.url);
});

test('a call is the caller its token says, whatever it claims', async (t) => {
  const s = await setup(t);
  const other = s.router.issue(OTHER_BOT, OTHER_SESSION);
  await rpc(s.port, s.token, call(1, 'list_bots', {}));
  await rpc(s.port, other, call(2, 'list_bots', {}));
  await rpc(s.port, s.token, call(3, 'get_task', { task_id: TASK }), { 'X-Bot-Id': OTHER_BOT });
  assert.deepEqual(s.calls.map((entry) => entry.caller), [
    { botId: BOT, sessionId: SESSION }, { botId: OTHER_BOT, sessionId: OTHER_SESSION }, { botId: BOT, sessionId: SESSION },
  ]);
  const withClaim = await rpc(s.port, s.token, call(4, 'delegate_task', { bot: 'x', goal: 'y', caller: OTHER_BOT }));
  assert.equal(JSON.parse(withClaim.text).result.isError, true, 'a call cannot name itself another caller: unknown arguments are refused');
  assert.equal(s.calls.length, 3);
});

test('without a token the router answers 401 and nothing else; so does a token it does not know', async (t) => {
  const s = await setup(t);
  const body = JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'ping' });
  for (const [label, token] of [['none', null], ['wrong', 'x'.repeat(43)], ['short', 'abc'], ['long', 'x'.repeat(200)], ['revoked', 'revoked']] as Array<[string, string | null]>) {
    const reply = await send(s.port, { token, headers: JSON_HEADERS, body });
    assert.equal(reply.status, 401, label);
    assert.equal(reply.headers['www-authenticate'], 'Bearer');
    assert.equal(reply.text, 'Unauthorized');
  }
  const other = s.router.issue(OTHER_BOT, OTHER_SESSION);
  assert.equal((await send(s.port, { headers: { ...JSON_HEADERS, Authorization: `Basic ${s.token}` }, body })).status, 401, 'only the bearer scheme');
  assert.equal((await send(s.port, { headers: { ...JSON_HEADERS, Authorization: s.token }, body })).status, 401);
  assert.equal((await send(s.port, { headers: { ...JSON_HEADERS, Authorization: `Bearer  ${s.token}` }, body })).status, 401, 'exactly one space');
  assert.equal((await send(s.port, { headers: JSON_HEADERS, token: other, body })).status, 200);
  assert.deepEqual(s.calls, []);
});

test('a token lives until its session\'s runtime is gone: replaced by a new launch, revoked with the session or with the Bot', async (t) => {
  const s = await setup(t);
  const ping = { jsonrpc: '2.0', id: 1, method: 'ping' };
  const status = async (token: string) => (await rpc(s.port, token, ping)).status;
  assert.match(s.token, /^[A-Za-z0-9_-]{43}$/, '32 random bytes');
  assert.equal(await status(s.token), 200);
  const relaunched = s.router.issue(BOT, SESSION);
  assert.notEqual(relaunched, s.token);
  assert.equal(await status(s.token), 401, 'the session\'s earlier token stops working when its engine is launched again');
  assert.equal(await status(relaunched), 200);
  assert.equal(s.router.issued, 1);
  s.router.revoke(SESSION);
  assert.equal(await status(relaunched), 401);
  const second = s.router.issue(BOT, SESSION);
  const third = s.router.issue(BOT, OTHER_SESSION);
  const foreign = s.router.issue(OTHER_BOT, '33333333-2222-4333-8444-555555555555');
  s.router.revokeBot(BOT);
  assert.deepEqual([await status(second), await status(third), await status(foreign)], [401, 401, 200], 'only the Bot\'s own');
  s.router.revoke('unknown');
  assert.equal(s.router.issued, 1);
  assert.notEqual(s.router.issue(BOT, SESSION), s.router.issue(BOT, SESSION));
});

test('every token is checked the same way: tokens that share a prefix with a good one are no better than any other', async (t) => {
  const s = await setup(t);
  for (let index = 0; index < 5; index += 1) s.router.issue(OTHER_BOT, `0000000${index}-2222-4333-8444-555555555555`);
  const almost = `${s.token.slice(0, 42)}${s.token.endsWith('A') ? 'B' : 'A'}`;
  assert.equal((await rpc(s.port, almost, { jsonrpc: '2.0', id: 1, method: 'ping' })).status, 401);
  assert.equal((await rpc(s.port, s.token, { jsonrpc: '2.0', id: 1, method: 'ping' })).status, 200);
});

test('the Host must be the router\'s own address: a name that resolves to it from a web page is refused', async (t) => {
  const s = await setup(t);
  const body = JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'ping' });
  for (const host of ['evil.example', `localhost:${s.port}`, `evil.example:${s.port}`, '127.0.0.1', `127.0.0.1:${Number(s.port) + 1}`, `[::1]:${s.port}`]) {
    const reply = await send(s.port, { token: s.token, headers: { ...JSON_HEADERS, Host: host }, body });
    assert.equal(reply.status, 403, host);
  }
  assert.equal((await send(s.port, { token: s.token, headers: { ...JSON_HEADERS, Host: `127.0.0.1:${s.port}` }, body })).status, 200);
});

test('a request that carries an Origin is a web page\'s, and is refused even with a good token', async (t) => {
  const s = await setup(t);
  const reply = await rpc(s.port, s.token, { jsonrpc: '2.0', id: 1, method: 'ping' }, { Origin: 'https://evil.example' });
  assert.equal(reply.status, 403);
  assert.equal((await rpc(s.port, s.token, { jsonrpc: '2.0', id: 1, method: 'ping' }, { Origin: 'null' })).status, 403);
  assert.deepEqual(s.calls, []);
});

test('only POST to /mcp is answered, and no answer ever carries a CORS header', async (t) => {
  const s = await setup(t);
  const replies: Reply[] = [];
  for (const method of ['GET', 'PUT', 'DELETE', 'PATCH', 'OPTIONS', 'HEAD']) {
    const reply = await send(s.port, { method, token: s.token, headers: { Origin: 'https://evil.example', 'Access-Control-Request-Method': 'POST' } });
    assert.equal(reply.status, 405, method);
    assert.equal(reply.headers['allow'], 'POST');
    replies.push(reply);
  }
  for (const path of ['/', '/mcp/', '/mcp?x=1', '/MCP', '/mcp/extra', '/health', '//mcp']) {
    const reply = await send(s.port, { path, token: s.token, headers: JSON_HEADERS, body: '{}' });
    assert.equal(reply.status, 404, path);
    replies.push(reply);
  }
  replies.push(await send(s.port, { token: null, headers: JSON_HEADERS, body: '{}' }), await rpc(s.port, s.token, { jsonrpc: '2.0', id: 1, method: 'ping' }));
  for (const reply of replies) {
    assert.deepEqual(Object.keys(reply.headers).filter((name) => name.startsWith('access-control-')), [], 'no CORS header');
  }
});

test('the body must be JSON, and not too large', async (t) => {
  const s = await setup(t, { maxRequestBytes: 1024 });
  const ping = (padding: number) => JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'ping', params: { padding: 'x'.repeat(padding) } });
  assert.equal((await send(s.port, { token: s.token, headers: { 'Content-Type': 'text/plain' }, body: ping(0) })).status, 415);
  assert.equal((await send(s.port, { token: s.token, body: ping(0) })).status, 415, 'no content type');
  assert.equal((await send(s.port, { token: s.token, headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: 'a=b' })).status, 415);
  assert.equal((await send(s.port, { token: s.token, headers: { 'Content-Type': 'application/jsonx' }, body: ping(0) })).status, 415);
  assert.equal((await send(s.port, { token: s.token, headers: { 'Content-Type': 'Application/JSON; charset=utf-8' }, body: ping(0) })).status, 200);
  const exact = ping(0).length;
  assert.equal((await rpc(s.port, s.token, JSON.parse(ping(1024 - exact - 11)))).status, 200, 'a body at the limit is read');
  const big = await send(s.port, { token: s.token, headers: JSON_HEADERS, body: ping(2000) });
  assert.equal(big.status, 413);
  const huge = await send(s.port, { token: s.token, headers: { ...JSON_HEADERS, 'Transfer-Encoding': 'chunked' }, body: ping(200_000) });
  assert.equal(huge.status, 413, 'also when the size is not declared');
  assert.equal((await rpc(s.port, s.token, { jsonrpc: '2.0', id: 2, method: 'ping' })).status, 200, 'and the router carries on');
});

test('what is not a JSON-RPC request is answered with the JSON-RPC error for it', async (t) => {
  const s = await setup(t);
  const bodyOf = (reply: Reply) => JSON.parse(reply.text);
  const garbled = await send(s.port, { token: s.token, headers: JSON_HEADERS, body: '{"jsonrpc":' });
  assert.equal(garbled.status, 400);
  assert.deepEqual(bodyOf(garbled), { jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Parse error' } });
  for (const [label, message] of [
    ['a batch', [{ jsonrpc: '2.0', id: 1, method: 'ping' }]],
    ['no version', { id: 1, method: 'ping' }],
    ['another version', { jsonrpc: '1.0', id: 1, method: 'ping' }],
    ['no method', { jsonrpc: '2.0', id: 1 }],
    ['a method that is not text', { jsonrpc: '2.0', id: 1, method: 5 }],
    ['an id that is null', { jsonrpc: '2.0', id: null, method: 'ping' }],
    ['an id that is an object', { jsonrpc: '2.0', id: {}, method: 'ping' }],
    ['a number', 7],
    ['null', null],
    ['a string', 'ping'],
  ] as Array<[string, unknown]>) {
    const reply = await rpc(s.port, s.token, message);
    assert.equal(reply.status, 400, label);
    assert.equal(bodyOf(reply).error.code, -32600, label);
    assert.equal(bodyOf(reply).id, null);
  }
  assert.equal(bodyOf(await rpc(s.port, s.token, [{ jsonrpc: '2.0', id: 1, method: 'ping' }])).error.message, 'Batches are not supported');
  assert.equal((await send(s.port, { token: s.token, headers: { ...JSON_HEADERS, 'MCP-Protocol-Version': '1999-01-01' }, body: '{"jsonrpc":"2.0","id":1,"method":"ping"}' })).status, 400);
  for (const version of SUPPORTED_PROTOCOLS) {
    assert.equal((await send(s.port, { token: s.token, headers: { ...JSON_HEADERS, 'MCP-Protocol-Version': version }, body: '{"jsonrpc":"2.0","id":1,"method":"ping"}' })).status, 200, version);
  }
});

test('initialize negotiates a protocol revision and says what the router is; the engine\'s own probes are told no', async (t) => {
  const s = await setup(t);
  const initialize = async (protocolVersion: unknown) => JSON.parse((await rpc(s.port, s.token, { jsonrpc: '2.0', id: 'init', method: 'initialize', params: { protocolVersion, capabilities: {}, clientInfo: { name: 'engine', version: '1' } } })).text);
  for (const version of SUPPORTED_PROTOCOLS) assert.equal((await initialize(version)).result.protocolVersion, version);
  const unknown = await initialize('2020-01-01');
  assert.equal(unknown.id, 'init');
  assert.equal(unknown.result.protocolVersion, SUPPORTED_PROTOCOLS[0], 'an older client is offered the newest the router speaks');
  assert.deepEqual(unknown.result.capabilities, { tools: { listChanged: false } }, 'tools only: no resources, no prompts');
  assert.deepEqual(unknown.result.serverInfo, { name: 'bots', version: '1.0.0' });
  assert.equal(typeof unknown.result.instructions, 'string');
  assert.match(unknown.result.instructions, /never try to answer for the owner/);
  assert.equal(JSON.parse((await rpc(s.port, s.token, { jsonrpc: '2.0', id: 1, method: 'initialize' })).text).result.protocolVersion, SUPPORTED_PROTOCOLS[0]);
  for (const method of ['server/discover', 'resources/list', 'prompts/list', 'logging/setLevel', 'tools/unknown', '']) {
    const reply = await rpc(s.port, s.token, { jsonrpc: '2.0', id: 9, method });
    assert.equal(reply.status, 200, 'a refusal of a method is a JSON-RPC error in an ordinary answer');
    assert.deepEqual(JSON.parse(reply.text), { jsonrpc: '2.0', id: 9, error: { code: -32601, message: 'Method not found' } }, method);
  }
});

test('notifications are accepted and answered with nothing; ping is answered', async (t) => {
  const s = await setup(t);
  for (const method of ['notifications/initialized', 'notifications/cancelled', 'anything/else']) {
    const reply = await rpc(s.port, s.token, { jsonrpc: '2.0', method, params: {} });
    assert.equal(reply.status, 202, method);
    assert.equal(reply.text, '');
  }
  assert.deepEqual(JSON.parse((await rpc(s.port, s.token, { jsonrpc: '2.0', id: 3, method: 'ping' })).text), { jsonrpc: '2.0', id: 3, result: {} });
  assert.deepEqual(s.calls, []);
});

test('tools/list offers the six tools, each with a schema that takes no argument it does not name', async (t) => {
  const s = await setup(t);
  const { result } = JSON.parse((await rpc(s.port, s.token, { jsonrpc: '2.0', id: 1, method: 'tools/list' })).text);
  assert.deepEqual(result.tools.map((tool: { name: string }) => tool.name), ['list_bots', 'delegate_task', 'get_task', 'message_bot', 'cancel_task', 'attach_artifact']);
  assert.deepEqual(result.tools.map((tool: { name: string }) => tool.name), [...ROUTER_TOOL_NAMES]);
  for (const tool of result.tools) {
    assert.equal(typeof tool.description, 'string');
    assert.equal(tool.inputSchema.type, 'object');
    assert.equal(tool.inputSchema.additionalProperties, false);
    for (const required of tool.inputSchema.required ?? []) assert.ok(required in tool.inputSchema.properties, `${tool.name}: ${required}`);
  }
});

test('a tool call reaches the backend with its arguments read strictly, and what it answers goes back as both text and structure', async (t) => {
  const s = await setup(t);
  const delegated = JSON.parse((await rpc(s.port, s.token, call(1, 'delegate_task', { bot: 'Other', goal: 'Do it', inputs: ['a.txt'], deadline_minutes: 5, idempotency_key: 'k1' }))).text).result;
  assert.equal(delegated.isError, undefined);
  assert.deepEqual(delegated.structuredContent.task_id, TASK);
  assert.equal(delegated.structuredContent.status.state, 'working');
  assert.deepEqual(JSON.parse(delegated.content[0].text), delegated.structuredContent);
  assert.deepEqual(s.calls[0]!.args, [{ bot: 'Other', goal: 'Do it', inputs: ['a.txt'], deadlineMinutes: 5, idempotencyKey: 'k1' }]);
  await rpc(s.port, s.token, call(2, 'message_bot', { task_id: TASK, text: 'More' }));
  await rpc(s.port, s.token, call(3, 'cancel_task', { task_id: TASK }));
  const attached = JSON.parse((await rpc(s.port, s.token, call(4, 'attach_artifact', { task_id: TASK, path: 'out/report.md' }))).text).result;
  assert.equal(attached.structuredContent.artifact.name, 'a');
  assert.deepEqual(s.calls.slice(1).map((entry) => [entry.method, ...entry.args]), [['messageBot', TASK, 'More'], ['cancelTask', TASK], ['attachArtifact', TASK, 'out/report.md']]);
  const listed = JSON.parse((await rpc(s.port, s.token, call(5, 'list_bots', undefined))).text).result;
  assert.equal(listed.structuredContent.bots[0].accepts_tasks_from_you, true);
});

test('arguments that do not fit are a result the Bot can read, not a protocol error', async (t) => {
  const s = await setup(t);
  const bad = async (name: string, args: unknown) => {
    const reply = JSON.parse((await rpc(s.port, s.token, call(1, name, args))).text);
    assert.equal(reply.error, undefined, `${name} ${JSON.stringify(args)}`);
    assert.equal(reply.result.isError, true, `${name} ${JSON.stringify(args)}`);
    assert.equal(reply.result.structuredContent.error.code, 'invalid');
    return reply.result.content[0].text as string;
  };
  assert.match(await bad('delegate_task', { goal: 'x' }), /bot must be a non-empty string/);
  assert.match(await bad('delegate_task', { bot: 'x' }), /goal must be a non-empty string/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: '   ' }), /goal must be/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', inputs: 'a.txt' }), /inputs must be a list/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', inputs: Array.from({ length: 17 }, () => 'a') }), /inputs must be a list/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', inputs: [''] }), /inputs must be a list/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', deadline_minutes: 0 }), /deadline_minutes must be a whole number from 1 to 1440/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', deadline_minutes: 1441 }), /deadline_minutes/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', deadline_minutes: 1.5 }), /deadline_minutes/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', deadline_minutes: '5' }), /deadline_minutes/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', idempotency_key: '' }), /idempotency_key/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', idempotency_key: 'k'.repeat(129) }), /idempotency_key/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y', extra: 1 }), /Unknown argument: extra/);
  assert.match(await bad('delegate_task', { bot: 'x', goal: 'y'.repeat(20_001) }), /goal must be/);
  assert.match(await bad('delegate_task', 'text'), /must be an object/);
  assert.match(await bad('delegate_task', ['bot']), /must be an object/);
  assert.match(await bad('get_task', {}), /task_id must be a task_id/);
  assert.match(await bad('get_task', { task_id: 'short' }), /task_id must be a task_id/);
  assert.match(await bad('get_task', { task_id: TASK, wait_seconds: 121 }), /wait_seconds must be a whole number from 0 to 120/);
  assert.match(await bad('get_task', { task_id: TASK, wait_seconds: -1 }), /wait_seconds/);
  assert.match(await bad('message_bot', { task_id: TASK }), /text must be/);
  assert.match(await bad('message_bot', { task_id: TASK, text: 'x'.repeat(8001) }), /text must be/);
  assert.match(await bad('cancel_task', { task_id: TASK, force: true }), /Unknown argument: force/);
  assert.match(await bad('attach_artifact', { task_id: TASK }), /path must be/);
  assert.match(await bad('attach_artifact', { task_id: TASK, path: 'a\0b' }), /path must be/);
  assert.match(await bad('list_bots', { all: true }), /Unknown argument: all/);
  assert.deepEqual(s.calls, [], 'none of them reached the backend');
});

test('a tool that does not exist is a protocol error; params that are not an object, too', async (t) => {
  const s = await setup(t);
  for (const params of [{ name: 'rm_rf', arguments: {} }, { name: 5 }, {}, { arguments: {} }, 'list_bots', null, { name: '__proto__' }, { name: 'constructor' }]) {
    const reply = JSON.parse((await rpc(s.port, s.token, { jsonrpc: '2.0', id: 1, method: 'tools/call', params })).text);
    assert.equal(reply.error.code, -32602, JSON.stringify(params));
    assert.equal(reply.result, undefined);
  }
  assert.deepEqual(s.calls, []);
});

test('a backend that fails with something other than a refusal is the host\'s trouble, which the Bot is not told about', async (t) => {
  const s = await setup(t);
  s.hooks.getTask = () => Promise.reject(new Error('EACCES: /Users/someone/secret'));
  const reply = JSON.parse((await rpc(s.port, s.token, call(1, 'get_task', { task_id: TASK }))).text).result;
  assert.equal(reply.isError, true);
  assert.equal(reply.content[0].text, 'The Bot router could not do that.');
  assert.deepEqual(reply.structuredContent.error, { code: 'internal', message: 'The Bot router could not do that.' });
  assert.equal(JSON.stringify(reply).includes('secret'), false);
  assert.match(s.warnings.join('\n'), /get_task failed: EACCES/);
});

test('a get_task that may wait is answered as an event stream whose headers go out at once', async (t) => {
  const s = await setup(t);
  let finish!: (task: TaskView) => void;
  s.hooks.getTask = () => new Promise<TaskView>((resolve) => { finish = resolve; });
  const seen: string[] = [];
  const reply = await new Promise<Reply>((resolve, reject) => {
    const req = request({ host: '127.0.0.1', port: Number(s.port), path: '/mcp', method: 'POST', headers: { ...JSON_HEADERS, Authorization: `Bearer ${s.token}` } }, (res) => {
      seen.push(`headers ${res.statusCode} ${res.headers['content-type']}`);
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () => resolve({ status: res.statusCode ?? 0, headers: res.headers, text: Buffer.concat(chunks).toString('utf8') }));
      setTimeout(() => { seen.push('finishing'); finish(view()); }, 50);
    });
    req.on('error', reject);
    req.end(JSON.stringify(call(1, 'get_task', { task_id: TASK, wait_seconds: 90 })));
  });
  assert.deepEqual(seen, ['headers 200 text/event-stream', 'finishing'], 'before the task had changed');
  assert.match(reply.text, /^event: message\ndata: \{.*\}\n\n$/s);
  const data = JSON.parse(reply.text.split('\n')[1]!.slice('data: '.length));
  assert.equal(data.id, 1);
  assert.equal(data.result.structuredContent.task.task_id, TASK);
  assert.deepEqual(s.calls[0]!.args, [TASK, 90]);
});

test('a client that cannot read an event stream is never kept waiting for more than twenty seconds', async (t) => {
  const s = await setup(t);
  const reply = await send(s.port, { token: s.token, headers: { 'Content-Type': 'application/json', Accept: 'application/json' }, body: JSON.stringify(call(1, 'get_task', { task_id: TASK, wait_seconds: 120 })) });
  assert.equal(reply.headers['content-type'], 'application/json');
  assert.equal(JSON.parse(reply.text).result.structuredContent.task.task_id, TASK);
  assert.deepEqual(s.calls[0]!.args, [TASK, 20]);
  await send(s.port, { token: s.token, headers: { 'Content-Type': 'application/json', Accept: 'application/json' }, body: JSON.stringify(call(2, 'get_task', { task_id: TASK, wait_seconds: 7 })) });
  assert.deepEqual(s.calls[1]!.args, [TASK, 7]);
  await rpc(s.port, s.token, call(3, 'get_task', { task_id: TASK }));
  assert.equal(s.calls[2]!.args[1], 0, 'asking at once is answered at once, as plain JSON');
});

test('a caller that hangs up is told so, and so is every waiting caller when the router stops', async (t) => {
  const s = await setup(t);
  const signals: AbortSignal[] = [];
  s.hooks.getTask = (_wait, signal) => new Promise<TaskView>((resolve) => {
    signals.push(signal);
    signal.addEventListener('abort', () => resolve(view()), { once: true });
  });
  const hangUp = await new Promise<void>((resolve, reject) => {
    const req = request({ host: '127.0.0.1', port: Number(s.port), path: '/mcp', method: 'POST', headers: { ...JSON_HEADERS, Authorization: `Bearer ${s.token}` } }, (res) => {
      res.resume();
      setTimeout(() => { req.destroy(); resolve(); }, 30);
    });
    req.on('error', () => undefined);
    req.on('close', () => undefined);
    req.end(JSON.stringify(call(1, 'get_task', { task_id: TASK, wait_seconds: 60 })));
    setTimeout(() => reject(new Error('no answer')), 2000).unref();
  });
  await hangUp;
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(signals[0]!.aborted, true);

  const waiting = send(s.port, { token: s.token, headers: JSON_HEADERS, body: JSON.stringify(call(2, 'get_task', { task_id: TASK, wait_seconds: 60 })) }).catch(() => undefined);
  await new Promise((resolve) => setTimeout(resolve, 30));
  await s.router.dispose();
  assert.equal(signals[1]!.aborted, true);
  await waiting;
  assert.equal(s.router.issued, 0, 'no token outlives the router');
  await assert.rejects(send(s.port, { token: s.token, headers: JSON_HEADERS, body: '{}' }), /ECONNREFUSED/);
});

/** Runtimes as the manager makes them: announced to whoever watches, from before they are started, and each says when it is gone. */
class FakeRuntime implements WatchedRuntime {
  private readonly observers = new Set<RemoteObserver>();
  constructor(readonly sessionId: string) {}
  observeRemote(observer: RemoteObserver): () => void {
    this.observers.add(observer);
    return () => { this.observers.delete(observer); };
  }
  gone(): void { for (const observer of [...this.observers]) observer({ type: 'disposed' }); }
}

function runtimeSource() {
  const listeners = new Set<(runtime: WatchedRuntime) => void>();
  const made: FakeRuntime[] = [];
  const source: RuntimeSource = {
    onRuntimeCreated(listener) {
      listeners.add(listener);
      for (const runtime of made) listener(runtime);
      return () => { listeners.delete(listener); };
    },
  };
  return {
    source, watchers: () => listeners.size,
    make(sessionId: string): FakeRuntime {
      const runtime = new FakeRuntime(sessionId);
      made.push(runtime);
      for (const listener of [...listeners]) listener(runtime);
      return runtime;
    },
  };
}

test('a session\'s token is revoked when its runtime is disposed, whether the runtime was there when the router began to watch or came after', async (t) => {
  const s = await setup(t);
  const status = async (token: string) => (await rpc(s.port, token, { jsonrpc: '2.0', id: 1, method: 'ping' })).status;
  const runtimes = runtimeSource();
  const before = runtimes.make(SESSION);
  const stop = s.router.watchRuntimes(runtimes.source);
  const after = runtimes.make(OTHER_SESSION);
  const other = s.router.issue(OTHER_BOT, OTHER_SESSION);
  assert.deepEqual([await status(s.token), await status(other)], [200, 200]);
  before.gone();
  assert.deepEqual([await status(s.token), await status(other)], [401, 200], 'only the session whose runtime is gone');
  after.gone();
  assert.equal(await status(other), 401);
  assert.equal(runtimes.watchers(), 1);
  stop();
  assert.equal(runtimes.watchers(), 0, 'and the router can stop watching');
  const unwatched = runtimes.make(SESSION);
  const token = s.router.issue(BOT, SESSION);
  unwatched.gone();
  assert.equal(await status(token), 200, 'a runtime that is not watched revokes nothing');
});

test('a runtime that is still going away when its session is opened again does not take the new token with it', async (t) => {
  const s = await setup(t);
  const status = async (token: string) => (await rpc(s.port, token, { jsonrpc: '2.0', id: 1, method: 'ping' })).status;
  const runtimes = runtimeSource();
  s.router.watchRuntimes(runtimes.source);
  const old = runtimes.make(SESSION);
  const fresh = runtimes.make(SESSION);
  const token = s.router.issue(BOT, SESSION);
  old.gone();
  old.gone();
  assert.equal(await status(token), 200, 'the new engine\'s token stands');
  fresh.gone();
  assert.equal(await status(token), 401, 'until the runtime that has it is gone');
});

test('a router that cannot listen says so, and the next call tries again', async (t) => {
  const blocker = createServer();
  await new Promise<void>((resolve) => blocker.listen(0, '127.0.0.1', resolve));
  const { port } = blocker.address() as AddressInfo;
  const router = new BotRouter({ backend: fakeBackend().backend, port });
  t.after(async () => { await router.dispose(); blocker.close(); });
  await assert.rejects(router.start(), /EADDRINUSE/);
  await assert.rejects(router.ready(), /EADDRINUSE/, 'still in the way, and asked again');
  await new Promise<void>((resolve) => blocker.close(() => resolve()));
  assert.equal(await router.start(), `http://127.0.0.1:${port}/mcp`);
  assert.equal(await router.ready(), `http://127.0.0.1:${port}/mcp`);
});

test('a router stopped while it starts leaves nothing listening, and does not start again', async () => {
  const router = new BotRouter({ backend: fakeBackend().backend });
  const starting = router.start();
  await router.dispose();
  const { port } = new URL(await starting);
  await assert.rejects(send(port, { token: null, body: '{}' }), /ECONNREFUSED/);
  await assert.rejects(router.start(), /has stopped/);
  await assert.rejects(router.ready(), /has stopped/);
  await router.dispose();
});
