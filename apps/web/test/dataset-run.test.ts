import assert from 'node:assert/strict';
import test from 'node:test';
import { DatasetRunSession } from '../../../packages/teams/dataset-run.js';

const approval = { permit: 'permit', planDigest: 'digest', credential: 'synthetic' };
const execution = { executor: 'teamBot', task: 'task', logicalRun: 'persistent-run', recipeSha256: 'recipe' } as const;

test('Dataset run advances more than 60 snapshot pages without the idle backoff', async () => {
  let calls = 0;
  const run = await DatasetRunSession.open('https://example.test', approval, execution, {
    fetch: async (_, options) => {
      assert.equal(JSON.parse(String(options?.body)).logicalRun, execution.logicalRun);
      calls++;
      return Response.json(calls <= 200 ? { status: 'preparing', blocks: calls - 1 } : {
        inputs: [{ kind: 'snapshot', dataset: 'orders', snapshot: 'frozen' }],
        automaticSteps: [], outputBindings: [], credential: 'synthetic-run', expires: Date.now() / 1000 + 60,
      });
    },
  });
  assert.equal(calls, 201);
  assert.equal(run.inputs[0].kind, 'snapshot');
});
