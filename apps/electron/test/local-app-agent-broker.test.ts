import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalAppAgentBroker, type LocalAppAgentAuthorization, type LocalAppAgentBrokerOptions, type LocalAppAgentExecution, type LocalAppAgentOwner } from '../src/main/local-apps/agentBroker.ts';
import { agentDigest, parseAgentRunInput, sameAgentGrant } from '../src/main/local-apps/agentPolicy.ts';
import type { LocalAppAgentEvent, LocalAppAgentRun, LocalAppAgentToolGrant } from '../src/shared/localAppAgent.ts';

const principal: LocalAppAgentOwner = { appId: 'a11111111111111111111', revision: 1, invocationId: 'invocation-1', webContentsId: 123 };
const other: LocalAppAgentOwner = { appId: 'a22222222222222222222', revision: 1, invocationId: 'invocation-2' };
const read: LocalAppAgentToolGrant = { id: 'builtin:Read', kind: 'builtin', name: 'Read' };
const nutrition: LocalAppAgentToolGrant = { id: 'mcp:nutrition:search', kind: 'mcp', serverConfigId: 'nutrition', configRevision: 'config-1', rawToolName: 'search_food', definitionDigest: 'definition-1' };
const input = { requestId: 'photo-1', prompt: 'Describe the supplied test object.' };
function auth(): LocalAppAgentAuthorization {
  return { settings: { enabled: true, model: { providerId: 'test-provider', modelId: 'test-model' }, tools: [read, nutrition], background: false, maxTurns: 20, timeoutMs: 600_000 },
    requestedTools: [read.id, nutrition.id], model: { providerId: 'test-provider', modelId: 'test-model', label: 'Test', canSelect: true, supportsImages: true, supportsStructuredOutput: true } };
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
async function fixture(t: { after(callback: () => Promise<void>): void }, overrides: Partial<LocalAppAgentBrokerOptions> = {}) {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-agent-broker-'));
  const options: LocalAppAgentBrokerOptions = { root, authorize: async () => auth(), revalidate: async () => true, execute: async () => ({ result: { text: 'done' } }), ...overrides };
  const broker = new LocalAppAgentBroker(options);
  t.after(async () => {
    try { await broker.dispose(); }
    catch (error) { if ((error as { code?: string }).code !== 'ledger_invalid') throw error; }
    finally { await fs.rm(root, { recursive: true, force: true }); }
  });
  return { root, broker, options };
}
async function terminal(broker: LocalAppAgentBroker, caller: LocalAppAgentOwner, runId: string): Promise<LocalAppAgentRun> {
  const done = deferred<LocalAppAgentRun>();
  const timeout = setTimeout(() => done.reject(new Error('Task did not finish')), 2000);
  const unsubscribe = await broker.subscribe(caller, runId, event => {
    if (event.type === 'state' && ['succeeded', 'failed', 'cancelled', 'interrupted'].includes(event.run.state)) done.resolve(event.run);
  });
  try { return await done.promise; } finally { clearTimeout(timeout); unsubscribe(); }
}

test('concurrent retries create one durable task and a changed request cannot execute', async t => {
  const started = deferred<LocalAppAgentExecution>(), completion = deferred<{ result: { text: string } }>();
  let calls = 0;
  const { broker } = await fixture(t, { execute: execution => { calls++; started.resolve(execution); return completion.promise; } });
  const [one, two] = await Promise.all([broker.run(principal, input), broker.run(principal, input)]);
  assert.equal(one.runId, two.runId);
  await started.promise;
  assert.equal(calls, 1);
  await assert.rejects(broker.run(principal, { ...input, prompt: 'Different side effect' }), { code: 'request_conflict' });
  completion.resolve({ result: { text: 'done' } });
  assert.equal((await terminal(broker, principal, one.runId)).state, 'succeeded');
  assert.deepEqual(await broker.run(principal, input), one);
  assert.equal(calls, 1);
});

test('concurrency is per application across revisions while unrelated applications can execute', async t => {
  const release = deferred<{ result: { text: string } }>();
  const { broker } = await fixture(t, { execute: () => release.promise });
  const first = await broker.run(principal, input);
  await assert.rejects(broker.run(principal, { ...input, requestId: 'second' }), { code: 'busy' });
  await assert.rejects(broker.run({ ...principal, revision: 2 }, input), { code: 'busy' });
  const second = await broker.run(other, input);
  assert.notEqual(first.runId, second.runId);
  release.resolve({ result: { text: 'done' } });
  await Promise.all([terminal(broker, principal, first.runId), terminal(broker, other, second.runId)]);
});

test('application, revision and live invocation ownership protect reads, subscriptions and cancellation', async t => {
  const completion = deferred<{ result: { text: string } }>();
  const { broker } = await fixture(t, { execute: () => completion.promise });
  const { runId } = await broker.run(principal, input);
  for (const forged of [other, { ...principal, revision: 2 }, { ...principal, invocationId: 'new-invocation' }]) {
    await assert.rejects(broker.get(forged, runId), { code: 'not_found' });
    await assert.rejects(broker.subscribe(forged, runId, () => {}), { code: 'not_found' });
    await assert.rejects(broker.cancel(forged, runId), { code: 'not_found' });
  }
  // A page refresh does not replace the runtime invocation.
  assert.equal((await broker.get({ ...principal, webContentsId: 456 }, runId)).runId, runId);
  completion.resolve({ result: { text: 'done' } });
  await terminal(broker, principal, runId);
  assert.equal((await broker.get({ ...principal, invocationId: 'reopened-invocation' }, runId)).result?.text, 'done');
});

test('structured result, usage and ordered progress survive reload without persisting source input', async t => {
  const gate = deferred<void>(), started = deferred<LocalAppAgentExecution>();
  const { broker, root } = await fixture(t, { execute: async execution => {
    started.resolve(execution); await gate.promise;
    await execution.emit({ phase: 'thinking', text: 'Analyzing the supplied image' });
    return { result: { output: { color: 'red' } }, usage: { inputTokens: 11, outputTokens: 3, costUsd: 0.001, turns: 1 } };
  } });
  const request = { ...input, prompt: 'PRIVATE_INPUT_SENTINEL', outputSchema: { type: 'object', properties: { color: { type: 'string' } }, required: ['color'] } };
  const { runId } = await broker.run(principal, request);
  await started.promise;
  const events: LocalAppAgentEvent[] = [];
  const unsubscribe = await broker.subscribe(principal, runId, event => events.push(event));
  gate.resolve();
  const done = await terminal(broker, principal, runId);
  unsubscribe();
  assert.equal(done.state, 'succeeded');
  assert.deepEqual(done.result?.output, { color: 'red' });
  assert.equal(done.usage?.inputTokens, 11);
  assert.ok(events.some(event => event.type === 'progress'));
  assert.ok(events.every((event, index) => index === 0 || event.sequence > events[index - 1].sequence));
  const bytes = await fs.readFile(join(root, `${runId}.json`), 'utf8');
  assert.ok(!bytes.includes('PRIVATE_INPUT_SENTINEL'));
  assert.ok(!bytes.includes('webContentsId'));
  const replay: LocalAppAgentEvent[] = [];
  const stop = await broker.subscribe(principal, runId, event => replay.push(event), done.sequence - 1); stop();
  assert.equal(replay.length, 1);
  assert.equal(replay[0].type, 'state');
  assert.equal(replay[0].sequence, done.sequence);
});

test('restart interrupts a previously committed intent and never replays its effects', async t => {
  const { root, options, broker } = await fixture(t);
  await fs.writeFile(join(root, 'old-run.json'), JSON.stringify({ schema: 1, owner: principal, fingerprint: agentDigest(parseAgentRunInput(input)),
    run: { runId: 'old-run', requestId: input.requestId, state: 'running', sequence: 9, createdAt: 1, updatedAt: 1,
      model: auth().settings.model, limits: { maxTurns: 20, timeoutMs: 600_000 } } }));
  let calls = 0;
  options.execute = async () => { calls++; return { result: { text: 'should not run' } }; };
  await broker.initialize();
  const reopened = { ...principal, invocationId: 'restarted-app' };
  const result = await broker.get(reopened, 'old-run');
  assert.equal(result.state, 'interrupted');
  assert.equal(result.sequence, 10);
  assert.equal(result.error?.code, 'host_restarted');
  assert.deepEqual(await broker.run(reopened, input), { runId: 'old-run' });
  assert.equal(calls, 0);
  assert.equal(JSON.parse(await fs.readFile(join(root, 'old-run.json'), 'utf8')).run.state, 'interrupted');
});

test('cancel wins over a late successful executor and emits exactly one terminal state', async t => {
  const started = deferred<LocalAppAgentExecution>(), completion = deferred<{ result: { text: string } }>();
  const { broker } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const { runId } = await broker.run(principal, input), execution = await started.promise;
  const events: LocalAppAgentEvent[] = [];
  const stop = await broker.subscribe(principal, runId, event => events.push(event));
  const cancelling = broker.cancel(principal, runId);
  await terminal(broker, principal, runId);
  assert.equal(execution.signal.aborted, true);
  completion.resolve({ result: { text: 'too late' } });
  await cancelling;
  await broker.cancel(principal, runId);
  const done = await broker.get(principal, runId); stop();
  assert.equal(done.state, 'cancelled'); assert.equal(done.result, undefined);
  assert.equal(events.filter(event => event.type === 'state' && ['succeeded', 'failed', 'cancelled'].includes(event.run.state)).length, 1);
});

test('lifecycle revocation invalidates authorization already waiting before execution', async t => {
  const authorizing = deferred<void>(), authorization = deferred<LocalAppAgentAuthorization>();
  let calls = 0;
  const { broker } = await fixture(t, { authorize: async () => { authorizing.resolve(); return authorization.promise; }, execute: async () => { calls++; return { result: {} }; } });
  const pending = broker.run(principal, input);
  await authorizing.promise;
  const cancelling = broker.cancelApp(principal.appId, 'Revoked during approval');
  authorization.resolve(auth());
  await assert.rejects(pending, { code: 'permission_revoked' });
  await cancelling;
  assert.equal(calls, 0);
});

test('revocation during a trusted approval blocks subsequent tool dispatch', async t => {
  const started = deferred<LocalAppAgentExecution>(), proceed = deferred<void>();
  let invoked = false;
  const { broker } = await fixture(t, { execute: async execution => {
    started.resolve(execution); await execution.emit({ phase: 'waiting_for_permission' });
    await proceed.promise; await execution.assertAuthorized(); invoked = true;
    return { result: {} };
  } });
  const { runId } = await broker.run(principal, { ...input, tools: [nutrition.id] }); await started.promise;
  const cancelling = broker.cancelApp(principal.appId, 'MCP grant revoked'); proceed.resolve();
  await cancelling;
  assert.equal(invoked, false);
  assert.equal((await broker.get(principal, runId)).state, 'cancelled');
});

test('each run fixes a grant subset and rechecks current provenance before dispatch', async t => {
  const state = auth(), started = deferred<LocalAppAgentExecution>(), proceed = deferred<void>();
  let dispatched = false;
  const { broker } = await fixture(t, { authorize: async () => state,
    revalidate: async (_owner, snapshot) => snapshot.settings.tools.every(tool => state.settings.tools.some(current => sameAgentGrant(tool, current))),
    execute: async execution => { started.resolve(execution); await proceed.promise; await execution.assertAuthorized(); dispatched = true; return { result: {} }; } });
  const { runId } = await broker.run(principal, { ...input, tools: [nutrition.id] });
  const execution = await started.promise;
  assert.deepEqual(execution.authorization.settings.tools, [nutrition]);
  state.settings.tools.push({ id: 'mcp:new-tool', kind: 'builtin', name: 'Bash' });
  assert.deepEqual(execution.authorization.settings.tools, [nutrition]);
  state.settings.tools = [read, { ...nutrition, configRevision: 'changed-server-command' }];
  proceed.resolve();
  const done = await terminal(broker, principal, runId);
  assert.equal(dispatched, false); assert.equal(done.state, 'cancelled'); assert.equal(done.error?.code, 'permission_revoked');
});

test('MCP configuration identity, raw name and definition each invalidate an old grant', () => {
  assert.equal(sameAgentGrant(nutrition, { ...nutrition }), true);
  for (const field of ['id', 'serverConfigId', 'configRevision', 'rawToolName', 'definitionDigest']) {
    assert.equal(sameAgentGrant(nutrition, { ...nutrition, [field]: 'different' }), false, field);
  }
  assert.equal(sameAgentGrant(nutrition, { id: nutrition.id, kind: 'builtin', name: 'search_food' }), false);
});

test('requested tools require both manifest declaration and explicit grant; omitted tools are empty', async t => {
  const state = auth(), seen: LocalAppAgentExecution[] = [];
  const { broker } = await fixture(t, { authorize: async () => state, execute: async execution => { seen.push(execution); return { result: {} }; } });
  await assert.rejects(broker.run(principal, { ...input, tools: ['builtin:Bash'] }), { code: 'tool_not_granted' });
  state.requestedTools = [read.id];
  await assert.rejects(broker.run(principal, { ...input, tools: [nutrition.id] }), { code: 'tool_not_granted' });
  const { runId } = await broker.run(principal, input); await terminal(broker, principal, runId);
  assert.deepEqual(seen[0].authorization.settings.tools, []);
});

test('host limits are enforced and applications may only lower them', async t => {
  const seen = deferred<LocalAppAgentExecution>();
  const { broker } = await fixture(t, { execute: async execution => { seen.resolve(execution); return { result: {} }; } });
  for (const overrides of [{ maxTurns: 21 }, { timeoutMs: 600_001 }]) await assert.rejects(broker.run(principal, { ...input, ...overrides }), { code: 'limit_exceeded' });
  const { runId } = await broker.run(principal, { ...input, maxTurns: 3, timeoutMs: 5000 });
  const execution = await seen.promise;
  assert.equal(execution.authorization.settings.maxTurns, 3); assert.equal(execution.authorization.settings.timeoutMs, 5000);
  assert.deepEqual((await terminal(broker, principal, runId)).limits, { maxTurns: 3, timeoutMs: 5000 });
});

test('time budget ends a noncooperative executor without waiting for its promise', async t => {
  const started = deferred<LocalAppAgentExecution>(), completion = deferred<{ result: {} }>();
  const { broker } = await fixture(t, { execute: execution => { started.resolve(execution); return completion.promise; } });
  const { runId } = await broker.run(principal, { ...input, timeoutMs: 40 });
  const execution = await started.promise, done = await terminal(broker, principal, runId);
  assert.equal(done.state, 'failed'); assert.equal(done.error?.code, 'timeout'); assert.equal(execution.signal.aborted, true);
  await assert.rejects(broker.run(principal, { ...input, requestId: 'while-stopping' }), { code: 'busy' });
  completion.resolve({ result: {} });
});

test('unconfigured and incompatible models are rejected before execution without fallback', async t => {
  const state = auth(); let calls = 0;
  const { broker } = await fixture(t, { authorize: async () => state, execute: async () => { calls++; return { result: {} }; } });
  state.model.canSelect = false;
  await assert.rejects(broker.run(principal, input), { code: 'model_unavailable' });
  state.model.canSelect = true; state.model.supportsStructuredOutput = false;
  await assert.rejects(broker.run(principal, { ...input, outputSchema: { type: 'object' } }), { code: 'structured_output_unsupported' });
  state.model.supportsImages = false;
  await assert.rejects(broker.run(principal, { ...input, images: [{ media_type: 'image/png', base64: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==' }] }), { code: 'images_unsupported' });
  assert.equal(calls, 0);
});

test('structured output cannot silently succeed with an unvalidated Markdown result', async t => {
  const { broker } = await fixture(t, { execute: async () => ({ result: { text: '```json\n{"value":1}\n```' } }) });
  const { runId } = await broker.run(principal, { ...input, outputSchema: { type: 'object' } });
  const done = await terminal(broker, principal, runId);
  assert.equal(done.state, 'failed'); assert.equal(done.error?.code, 'invalid_result'); assert.equal(done.result, undefined);
});

test('models without tool capability reject builtin and MCP calls before admission but can run text tasks', async t => {
  const state = auth(); state.model.supportsStructuredOutput = false;
  let calls = 0;
  const { broker, root } = await fixture(t, { authorize: async () => state, execute: async () => { calls++; return { result: { text: 'Text remains supported' } }; } });
  for (const tool of [read, nutrition]) {
    await assert.rejects(broker.run(principal, { ...input, tools: [tool.id] }), { code: 'tools_unsupported' });
    assert.equal(calls, 0);
    assert.deepEqual(await fs.readdir(root), [], 'unsupported tool requests do not commit a task or consume the request ID');
  }
  const { runId } = await broker.run(principal, { ...input, tools: [] });
  assert.equal((await terminal(broker, principal, runId)).state, 'succeeded');
  assert.equal(calls, 1);
});

test('provider diagnostics and unexpected fields do not cross the application boundary', async t => {
  const { broker, root } = await fixture(t, { execute: async () => { throw new Error('Authorization: Bearer SECRET_PROVIDER_CREDENTIAL'); } });
  await assert.rejects(broker.run(principal, { ...input, apiKey: 'do-not-accept' }), { code: 'invalid_input' });
  const { runId } = await broker.run(principal, input);
  const done = await terminal(broker, principal, runId);
  assert.equal(done.error?.code, 'execution_failed');
  assert.ok(!JSON.stringify(done).includes('SECRET_PROVIDER_CREDENTIAL'));
  assert.ok(!(await fs.readFile(join(root, `${runId}.json`), 'utf8')).includes('SECRET_PROVIDER_CREDENTIAL'));
});

test('invalid images, external schema references and excessive schema depth are rejected', () => {
  assert.throws(() => parseAgentRunInput({ ...input, images: [{ path: '/private/photo.png' }] }), { code: 'invalid_image' });
  assert.throws(() => parseAgentRunInput({ ...input, images: [{ media_type: 'image/png', base64: Buffer.from('not an image').toString('base64') }] }), { code: 'invalid_image' });
  assert.throws(() => parseAgentRunInput({ ...input, outputSchema: { $ref: 'https://private.example/schema' } }), { code: 'invalid_schema' });
  const cycle: Record<string, unknown> = {}; cycle.properties = cycle;
  assert.throws(() => parseAgentRunInput({ ...input, outputSchema: cycle }), { code: 'invalid_schema' });
  assert.deepEqual(parseAgentRunInput({ ...input, outputSchema: { $ref: '#/definitions/Result', definitions: { Result: { type: 'object' } } } }).outputSchema?.$ref, '#/definitions/Result');
});

test('host shutdown interrupts active tasks and refuses new admission', async t => {
  const started = deferred<LocalAppAgentExecution>();
  const { broker } = await fixture(t, { execute: execution => { started.resolve(execution); return new Promise((_resolve, reject) => execution.signal.addEventListener('abort', () => reject(execution.signal.reason), { once: true })); } });
  const { runId } = await broker.run(principal, input); const execution = await started.promise;
  await broker.dispose();
  assert.equal(execution.signal.aborted, true); assert.equal((await broker.get(principal, runId)).state, 'interrupted');
  await assert.rejects(broker.run(principal, { ...input, requestId: 'new' }), { code: 'host_stopped' });
});

test('shutdown drains an authorization in progress before reporting completion', async t => {
  const authorizing = deferred<void>(), release = deferred<LocalAppAgentAuthorization>(); let calls = 0;
  const { broker } = await fixture(t, { authorize: async () => { authorizing.resolve(); return release.promise; }, execute: async () => { calls++; return { result: {} }; } });
  const pending = broker.run(principal, input); await authorizing.promise;
  let disposed = false;
  const stopping = broker.dispose().then(() => { disposed = true; });
  await new Promise(resolve => setImmediate(resolve)); assert.equal(disposed, false);
  release.resolve(auth());
  await assert.rejects(pending, { code: 'host_stopped' }); await stopping;
  assert.equal(calls, 0);
});

test('cancel waits for child shutdown and does not admit a second app task in the meantime', async t => {
  const started = deferred<void>(), closed = deferred<{ result: {} }>();
  const { broker } = await fixture(t, { execute: () => { started.resolve(); return closed.promise; } });
  const { runId } = await broker.run(principal, input); await started.promise;
  let cancellationReturned = false;
  const cancelling = broker.cancelApp(principal.appId, 'Replacing the runtime version').then(() => { cancellationReturned = true; });
  await terminal(broker, principal, runId);
  assert.equal(cancellationReturned, false);
  await assert.rejects(broker.run(principal, { ...input, requestId: 'new-version' }), { code: 'busy' });
  closed.resolve({ result: {} }); await cancelling;
  assert.equal(cancellationReturned, true);
});

test('a failed terminal ledger write cannot publish a false successful snapshot', async t => {
  const started = deferred<void>(), complete = deferred<{ result: { text: string } }>();
  const { broker, root } = await fixture(t, { execute: () => { started.resolve(); return complete.promise; } });
  const { runId } = await broker.run(principal, input); await started.promise;
  const rename = fs.rename;
  let fail = true;
  fs.rename = (async (oldPath, newPath) => {
    if (fail && String(newPath) === join(root, `${runId}.json`)) { fail = false; throw new Error('disk temporarily unavailable'); }
    return rename(oldPath, newPath);
  }) as typeof fs.rename;
  try {
    complete.resolve({ result: { text: 'do not publish before commit' } });
    const done = await terminal(broker, principal, runId);
    assert.equal(done.state, 'failed'); assert.equal(done.result, undefined);
    assert.equal(JSON.parse(await fs.readFile(join(root, `${runId}.json`), 'utf8')).run.state, 'failed');
  } finally { fs.rename = rename; }
});

test('a symlink ledger entry is refused rather than importing another app data', async t => {
  const { broker, root } = await fixture(t);
  const outside = join(root, 'private-data'); await fs.writeFile(outside, '{}');
  await fs.symlink(outside, join(root, 'forged.json'));
  await assert.rejects(broker.initialize(), { code: 'ledger_invalid' });
  // Cleanup hook calls dispose; remove the deliberately corrupt fixture and
  // use a separate broker to verify the original outside file was untouched.
  assert.equal(await fs.readFile(outside, 'utf8'), '{}');
});

test('an unexpected executor disconnection records interruption and never replays the request', async t => {
  const { LocalAppAgentError } = await import('../src/main/local-apps/agentPolicy.ts');
  let calls = 0;
  const { broker } = await fixture(t, { execute: async () => { calls++; throw new LocalAppAgentError('executor_closed', 'The Agent host exited before completing the task.'); } });
  const one = await broker.run(principal, input);
  assert.equal((await terminal(broker, principal, one.runId)).state, 'interrupted');
  assert.deepEqual(await broker.run(principal, input), one);
  assert.equal(calls, 1);
});

test('prompt size uses the same UTF-8 byte budget as the execution bridge', () => {
  assert.throws(() => parseAgentRunInput({ ...input, prompt: '图'.repeat(90_000) }), { code: 'invalid_input' });
  assert.equal(parseAgentRunInput({ ...input, prompt: '图'.repeat(80_000) }).prompt.length, 80_000);
});
