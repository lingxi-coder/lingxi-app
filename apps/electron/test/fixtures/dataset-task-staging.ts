import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { TestContext } from 'node:test';
import type { DatasetExecutor } from '../../../../packages/teams/dataset-run.js';
import { DatasetTaskHost, type DatasetTaskHooks, type DatasetTaskProgram } from '../../src/main/teams/datasetTasks.js';
import { datasetFixtureTransport } from './dataset-fixture-transport.js';

export const hasDatasetFixture = Boolean(process.env.DATASET_V2_TEST_ORIGIN && process.env.DATASET_V2_OWNER_FILE);

export async function datasetExecutorFixture(t: TestContext, executor: DatasetExecutor): Promise<{
  hooks: DatasetTaskHooks;
  authorize(program: DatasetTaskProgram): Promise<void>;
  assertOutput(expected: string): Promise<void>;
}> {
  const directory = await mkdtemp(join(tmpdir(), 'dataset-task-host-'));
  const origin = process.env.DATASET_V2_TEST_ORIGIN!, token = await readFile(process.env.DATASET_V2_OWNER_FILE!, 'utf8');
  const transport = await datasetFixtureTransport(origin);
  const uid = (JSON.parse(Buffer.from(token.split('.')[1]!, 'base64url').toString('utf8')) as { sub: string }).sub;
  const account = { snapshot: () => ({ user: { uid } }), getIdToken: async () => token, onChanged: () => () => {} };
  const store = new Map<string, string>();
  const vault = { write: async (id: string, value: string) => { store.set(id, value); },
    read: async (id: string) => store.get(id), remove: async (id: string) => { store.delete(id); } };
  async function command<T>(observedUid: string, body: Record<string, unknown>): Promise<T> {
    assert.equal(observedUid, uid);
    const response = await transport.fetch(origin + '/v1/teams/datasets', { method: 'POST', headers: { Authorization: 'Bearer ' + token,
      'Content-Type': 'application/json' }, body: JSON.stringify(body) });
    const result = await response.json();
    assert.equal(response.status, 200, JSON.stringify(result));
    return result as T;
  }
  const dataset = 'fixture-' + randomUUID(), host = new DatasetTaskHost(directory, origin, account, vault, command, transport.fetch);
  const call = <T>(cmd: Record<string, unknown>, operation?: string) => command<T>(uid, { protocol: 2, dataset, operation, command: cmd });
  await call({ action: 'create', schema: { type: 'object', additionalProperties: false, required: ['result'], properties: { result: { type: 'string' } } } }, 'create');
  await call({ action: 'put', record: 'seed', partition: 'test', expectedRevision: 0, schemaVersion: 1, value: { result: 'approved input' } }, 'seed');
  let program: DatasetTaskProgram | undefined;
  async function authorize(current: DatasetTaskProgram): Promise<void> {
    const result = await host.approve(current, dataset, { inputs: [{ kind: 'latestRecord', record: 'seed' }],
      sources: [{ kind: 'dataset', dataset }], metric: 'result', projection: { fields: ['result'], partitions: ['test'] },
      expires: Math.floor(Date.now() / 1000) + 3600, actions: { save: { kind: 'put', record: 'output', partition: 'test',
        schemaVersion: 1, fields: ['result'], fromOutput: { result: [executor === 'nativeSchedule' ? 'summary' : 'text'] } } } }, 'approval', uid);
    assert.deepEqual(Object.keys(result).sort(), ['permit', 'planDigest']);
    program = current;
  }
  const hooks: DatasetTaskHooks = {
    async open(current, logical, signal) {
      assert.equal(current.executor, executor);
      if (!program) await authorize(current);
      return host.open(current, logical, signal);
    },
  };
  t.after(async () => { if (program) await host.revoke(program, uid); await call({ action: 'delete' }); await rm(directory, { recursive: true, force: true }); });
  return { hooks, authorize, async assertOutput(expected: string) {
    const row = await call<{ value: { result: string }; reference: { kind: string; dataset: string; record: string; revision: number }; actor: { role: string } }>({ action: 'latest', record: 'output' });
    assert.equal(row.value.result, expected); assert.equal(row.reference.revision, 1); assert.equal(row.actor.role, 'task');
    const history = await call<{ outputs: { executor: DatasetExecutor }[] }>({ action: 'outputs' });
    assert.deepEqual(history.outputs.map(o => o.executor), [executor]);
    if (process.env.DATASET_V2_TEST_IAM_FILE) {
      assert.equal(transport.replayedWrites(), 1);
      t.diagnostic('DATASET_RECEIPT ' + JSON.stringify({ executor, reference: row.reference, attributedTo: row.actor.role,
        replaysCheckedOnDistinctReplicas: 2, persistedOutputs: history.outputs.length }));
    }
  } };
}
