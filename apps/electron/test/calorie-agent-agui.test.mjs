import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { MealAgentTask, revisionStorage } from '../../../fixtures/local-apps/v2/calorie-agent/agui-task.mjs';
import { nutritionSchema, recognitionPrompt } from '../../../fixtures/local-apps/v2/calorie-agent/nutrition.mjs';

const fixtureRoot = new URL('../../../fixtures/local-apps/v2/calorie-agent/', import.meta.url);

function memoryStorage() {
  const values = new Map();
  return {
    values,
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: key => values.delete(key),
  };
}

function makeAgent({ start, get, connect } = {}) {
  const calls = { starts: [], gets: [], cancels: [], clients: [] };
  const inputs = new Map();
  const agent = {
    calls,
    file: async file => {
      assert.ok(file instanceof File);
      return { type: 'image', id: 'part-1', source: { type: 'data', value: 'aGVsbG8=', mimeType: 'image/jpeg' } };
    },
    tasks: {
      start: async input => {
        calls.starts.push(structuredClone(input)); inputs.set(input.runId, structuredClone(input));
        return start ? start(input, calls) : { runId: input.runId };
      },
      get: async runId => {
        calls.gets.push(runId);
        return get ? get(runId, calls) : { runId, threadId: inputs.get(runId)?.threadId ?? 'thread-1', state: 'running', phase: 'thinking' };
      },
      events: async input => ({ events: [], sequence: input.afterSequence ?? 0 }),
      cancel: async runId => { calls.cancels.push(runId); },
    },
    createClient: options => {
      calls.clients.push(options);
      return {
        connectAgent: async ({ runId }, subscriber) => {
          if (connect) return connect({ runId, threadId: options.threadId }, subscriber, calls);
          await subscriber.onEvent({ event: { type: 'RUN_STARTED', threadId: options.threadId, runId } });
          await subscriber.onEvent({ event: { type: 'RUN_FINISHED', threadId: options.threadId, runId, result: { items: [], assumptions: [], questions: [] } } });
          return { result: { items: [], assumptions: [], questions: [] }, newMessages: [{ id: 'assistant-1', role: 'assistant', content: 'Done' }] };
        },
      };
    },
  };
  return agent;
}

function enumerableStorage(seed = {}) {
  const values = new Map(Object.entries(seed));
  return {
    values,
    get length() { return values.size; },
    key: index => [...values.keys()][index] ?? null,
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: key => values.delete(key),
  };
}

async function resumeWithEvents(connect) {
  const agent = makeAgent({
    get: async runId => ({ runId, threadId: 'saved-thread', state: 'running', phase: 'thinking' }),
    connect,
  });
  const storage = revisionStorage(memoryStorage(), 5);
  storage.setItem('meal-notes.agui-v1.run', JSON.stringify({ runId: 'saved-run', threadId: 'saved-thread' }));
  const errors = [];
  const tasks = [];
  const client = new MealAgentTask(agent, storage, {
    onTask: task => tasks.push(task.state),
    onError: error => errors.push(error),
  });
  return { agent, client, errors, tasks };
}

test('calorie fixture builds a standard AG-UI RunAgentInput with File helper and host extension', async () => {
  const agent = makeAgent();
  const task = new MealAgentTask(agent, memoryStorage());
  const file = new File([new Uint8Array([1, 2, 3])], 'meal-photo.jpg', { type: 'image/jpeg' });
  const input = await task.createInput(file, recognitionPrompt('No peanuts'), nutritionSchema, { maxTurns: 4, timeoutMs: 120000 });

  assert.match(input.threadId, /^[0-9a-f-]{36}$/i);
  assert.match(input.runId, /^[0-9a-f-]{36}$/i);
  assert.deepEqual(Object.keys(input).sort(), ['forwardedProps', 'messages', 'runId', 'threadId']);
  assert.equal(input.messages.length, 1);
  assert.equal(input.messages[0].role, 'user');
  assert.equal(input.messages[0].content[0].type, 'text');
  assert.equal(input.messages[0].content[1].type, 'image');
  assert.deepEqual(input.forwardedProps['com.lingxi.localApp'], {
    version: 1, outputSchema: nutritionSchema, tools: [], maxTurns: 4, timeoutMs: 120000,
  });
  assert.deepEqual(agent.calls.starts, []);
});

test('submission admits one official task and returns the SDK-assembled RunAgentResult', async () => {
  const rawStorage = memoryStorage();
  const storage = revisionStorage(rawStorage, 7);
  const agent = makeAgent();
  const events = [];
  const client = new MealAgentTask(agent, storage, { onEvent: event => events.push(event.type) });
  const input = await client.createInput(new File(['photo'], 'meal.jpg', { type: 'image/jpeg' }), 'Estimate this meal.', nutritionSchema);

  const run = await client.submit(input);

  assert.deepEqual(agent.calls.starts, [input]);
  assert.deepEqual(agent.calls.clients, [{ threadId: input.threadId }]);
  assert.deepEqual(events, ['RUN_STARTED', 'RUN_FINISHED']);
  assert.deepEqual(run.result, { items: [], assumptions: [], questions: [] });
  assert.equal(run.newMessages[0].role, 'assistant');
  assert.equal(client.pendingInput, null);
  assert.deepEqual(client.savedRun, { runId: input.runId, threadId: input.threadId });
  assert.equal([...rawStorage.values.keys()].some(key => key.includes('meal-notes.agent.')), false);
});

test('startup deletes only known obsolete Agent keys without decoding and preserves business and AG-UI data', () => {
  const newPending = {
    threadId: 'new-thread', runId: 'new-run',
    messages: [{ id: 'new-message', role: 'user', content: [] }],
  };
  const newRun = { runId: 'new-run', threadId: 'new-thread' };
  const rawStorage = enumerableStorage({
    'meal-notes.agent.pending': 'old photo payload that must not be decoded',
    'meal-notes.agent.run': 'old run id',
    'revision-2:meal-notes.agent.pending': 'old revision photo payload',
    'revision-5:meal-notes.agent.run': 'old revision run id',
    'revision-7:meal-notes.agent.pending': '{not valid JSON',
    'revision-7:meal-notes.agent.run': 'old current-revision run id',
    'revision-7:meal-notes.agui-v1.pending': JSON.stringify(newPending),
    'revision-7:meal-notes.agui-v1.run': JSON.stringify(newRun),
    'meal-notes.records.v1': '[{"name":"Apple","grams":120}]',
    'revision-7:meal-notes.records.v1': '[{"name":"Pear","grams":90}]',
    'revision-4:meal-notes.agent.archive': 'not a known obsolete key',
  });
  const client = new MealAgentTask(makeAgent(), revisionStorage(rawStorage, 7));

  assert.deepEqual(client.pendingInput, newPending);
  assert.deepEqual(client.savedRun, newRun);
  for (const key of [
    'meal-notes.agent.pending', 'meal-notes.agent.run',
    'revision-2:meal-notes.agent.pending', 'revision-5:meal-notes.agent.run',
    'revision-7:meal-notes.agent.pending', 'revision-7:meal-notes.agent.run',
  ]) assert.equal(rawStorage.getItem(key), null, `${key} should be removed`);
  assert.equal(rawStorage.getItem('revision-7:meal-notes.agui-v1.pending'), JSON.stringify(newPending));
  assert.equal(rawStorage.getItem('revision-7:meal-notes.agui-v1.run'), JSON.stringify(newRun));
  assert.equal(rawStorage.getItem('meal-notes.records.v1'), '[{"name":"Apple","grams":120}]');
  assert.equal(rawStorage.getItem('revision-7:meal-notes.records.v1'), '[{"name":"Pear","grams":90}]');
  assert.equal(rawStorage.getItem('revision-4:meal-notes.agent.archive'), 'not a known obsolete key');
});

test('obsolete-key cleanup works with a generic StorageLike mock without key or length', () => {
  const rawStorage = memoryStorage();
  rawStorage.values.set('meal-notes.agent.pending', 'invalid legacy data');
  rawStorage.values.set('revision-4:meal-notes.agent.run', 'old run');
  rawStorage.values.set('meal-notes.records.v1', '[{"name":"Peach"}]');
  const storage = revisionStorage(rawStorage, 4);

  assert.equal('length' in rawStorage, false);
  assert.equal(typeof rawStorage.key, 'undefined');
  assert.doesNotThrow(() => new MealAgentTask(makeAgent(), storage));
  assert.equal(rawStorage.getItem('meal-notes.agent.pending'), null);
  assert.equal(rawStorage.getItem('revision-4:meal-notes.agent.run'), null);
  assert.equal(rawStorage.getItem('meal-notes.records.v1'), '[{"name":"Peach"}]');
});

test('read-only reconnect rejects RUN_ERROR with its standard message and code', async () => {
  const { agent, client, errors, tasks } = await resumeWithEvents(async ({ runId, threadId }, subscriber) => {
    await subscriber.onEvent({ event: { type: 'RUN_STARTED', threadId, runId } });
    await subscriber.onEvent({ event: { type: 'RUN_ERROR', threadId, runId, message: 'Provider rejected this request.', code: 'model_tools_unsupported' } });
    return { result: { items: [] }, newMessages: [] };
  });

  let rejection;
  await assert.rejects(client.resume(), error => {
    rejection = error;
    assert.equal(error.message, 'Provider rejected this request.');
    assert.equal(error.code, 'model_tools_unsupported');
    assert.equal(error.event.type, 'RUN_ERROR');
    return true;
  });
  assert.deepEqual(agent.calls.starts, [], 'connectAgent must not start a new model run');
  assert.deepEqual(tasks, ['running', 'failed']);
  assert.deepEqual(errors, [rejection]);
});

test('read-only reconnect rejects cancelled and interrupted RUN_FINISHED outcomes', async t => {
  for (const { outcome, code, state } of [
    { outcome: { type: 'cancelled' }, code: 'run_cancelled', state: 'cancelled' },
    { outcome: { type: 'interrupt', interrupts: [{}] }, code: 'run_interrupted', state: 'interrupted' },
  ]) {
    await t.test(outcome.type, async () => {
      const { agent, client, errors, tasks } = await resumeWithEvents(async ({ runId, threadId }, subscriber) => {
        await subscriber.onEvent({ event: { type: 'RUN_STARTED', threadId, runId } });
        await subscriber.onEvent({ event: { type: 'RUN_FINISHED', threadId, runId, outcome, result: { items: [] } } });
        return { result: { items: [] }, newMessages: [{ id: 'assistant-1', role: 'assistant', content: 'looks successful' }] };
      });

      let rejection;
      await assert.rejects(client.resume(), error => {
        rejection = error;
        assert.equal(error.code, code);
        return true;
      });
      assert.deepEqual(agent.calls.starts, [], 'read-only reconnect must not start another run');
      assert.deepEqual(tasks, ['running', state]);
      assert.deepEqual(errors, [rejection]);
    });
  }
});

test('read-only reconnect accepts only success or an absent outcome', async t => {
  for (const outcome of [null, {}, { type: 'unknown' }]) {
    await t.test(JSON.stringify(outcome), async () => {
      const { agent, client, errors, tasks } = await resumeWithEvents(async ({ runId, threadId }, subscriber) => {
        await subscriber.onEvent({ event: { type: 'RUN_STARTED', threadId, runId } });
        await subscriber.onEvent({ event: { type: 'RUN_FINISHED', threadId, runId, outcome, result: { items: [] } } });
        return { result: { items: [] }, newMessages: [{ id: 'assistant-1', role: 'assistant', content: 'looks successful' }] };
      });

      let rejection;
      await assert.rejects(client.resume(), error => {
        rejection = error;
        assert.equal(error.code, 'invalid_terminal_outcome');
        return true;
      });
      assert.deepEqual(agent.calls.starts, []);
      assert.deepEqual(tasks, ['running', 'failed']);
      assert.deepEqual(errors, [rejection]);
    });
  }
});

test('read-only reconnect rejects a resolved SDK result when no terminal event was received', async () => {
  const { agent, client, errors, tasks } = await resumeWithEvents(async ({ runId, threadId }, subscriber) => {
    await subscriber.onEvent({ event: { type: 'RUN_STARTED', threadId, runId } });
    return { result: { items: [] }, newMessages: [{ id: 'assistant-1', role: 'assistant', content: 'looks successful' }] };
  });

  let rejection;
  await assert.rejects(client.resume(), error => {
    rejection = error;
    assert.equal(error.code, 'missing_terminal_event');
    return true;
  });
  assert.deepEqual(agent.calls.starts, []);
  assert.deepEqual(tasks, ['running', 'failed']);
  assert.deepEqual(errors, [rejection]);
});

test('read-only reconnect rejects multiple terminal events instead of accepting the first success', async () => {
  const { agent, client, errors, tasks } = await resumeWithEvents(async ({ runId, threadId }, subscriber) => {
    await subscriber.onEvent({ event: { type: 'RUN_STARTED', threadId, runId } });
    await subscriber.onEvent({ event: { type: 'RUN_FINISHED', threadId, runId } });
    await subscriber.onEvent({ event: { type: 'RUN_ERROR', threadId, runId, message: 'late error', code: 'late_error' } });
    return { result: { items: [] }, newMessages: [] };
  });

  let rejection;
  await assert.rejects(client.resume(), error => {
    rejection = error;
    assert.equal(error.code, 'multiple_terminal_events');
    return true;
  });
  assert.deepEqual(agent.calls.starts, []);
  assert.deepEqual(tasks, ['running', 'succeeded', 'failed']);
  assert.deepEqual(errors, [rejection]);
});

test('retry uses the same public run, thread and message IDs after an uncertain start response', async () => {
  let starts = 0;
  const agent = makeAgent({ start: async input => {
    starts += 1;
    if (starts === 1) throw new Error('response lost after admission');
    return { runId: input.runId };
  } });
  const storage = revisionStorage(memoryStorage(), 3);
  const client = new MealAgentTask(agent, storage);
  const original = await client.createInput(new File(['photo'], 'meal.jpg', { type: 'image/jpeg' }), 'Estimate this meal.', nutritionSchema);

  await assert.rejects(client.submit(original), /response lost/);
  const retry = { ...original, runId: 'a-different-run-id' };
  await client.submit(retry);

  assert.equal(agent.calls.starts.length, 2);
  assert.deepEqual(agent.calls.starts[0], original);
  assert.deepEqual(agent.calls.starts[1], original);
});

test('cold resume checks the recorded task identity and connects read-only without another start', async () => {
  const agent = makeAgent({ get: async runId => ({ runId, threadId: 'saved-thread', state: 'succeeded' }) });
  const storage = revisionStorage(memoryStorage(), 2);
  storage.setItem('meal-notes.agui-v1.run', JSON.stringify({ runId: 'saved-run', threadId: 'saved-thread' }));
  const client = new MealAgentTask(agent, storage);

  const run = await client.resume();

  assert.equal(agent.calls.starts.length, 0);
  assert.deepEqual(agent.calls.gets, ['saved-run']);
  assert.deepEqual(agent.calls.clients, [{ threadId: 'saved-thread' }]);
  assert.deepEqual(run.result, { items: [], assumptions: [], questions: [] });
});

test('the staged app no longer references the prototype Agent API or file', async () => {
  const app = await readFile(new URL('app.mjs', fixtureRoot), 'utf8');
  const taskHelper = await readFile(new URL('agui-task.mjs', fixtureRoot), 'utf8');
  const manifest = JSON.parse(await readFile(new URL('lingxi-app.json', fixtureRoot), 'utf8'));
  await assert.rejects(readFile(new URL('agent-client.mjs', fixtureRoot)));
  assert.ok(manifest.package.files.includes('agui-task.mjs'));
  assert.ok(!manifest.package.files.includes('agent-client.mjs'));
  assert.match(taskHelper, /agent\.file\(file\)/);
  assert.match(taskHelper, /agent\.tasks\.start/);
  assert.match(taskHelper, /client\.connectAgent\(\{ runId \}/);
  assert.match(app, /runResult\.result/);
  assert.doesNotMatch(app, /requestId:\s*crypto\.randomUUID|images:\s*\[image\]|\.pending\b|agent\.get\(|agent\.subscribe\(/);
});

test('cancel routes through the public task resource for the saved runId', async () => {
  const agent = makeAgent();
  const client = new MealAgentTask(agent, revisionStorage(memoryStorage(), 1));
  client.runId = 'cancel-run';
  client.threadId = 'cancel-thread';
  client.currentTask = { runId: 'cancel-run', threadId: 'cancel-thread', state: 'running' };

  await client.cancel();

  assert.deepEqual(agent.calls.cancels, ['cancel-run']);
});
