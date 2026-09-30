import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test, type TestContext } from 'node:test';
import type { ClientCommand, CronJobDto } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';

function claimTask(generation: number): CronJobDto {
  return { id: 'job', prompt: 'Scheduled task', automation: {
    runs: [{ id: 'occurrence', claimGeneration: generation, status: 'running' }],
  } } as unknown as CronJobDto;
}

async function controller(t: TestContext, nextGeneration = false) {
  const replies: ClientCommand[] = [];
  let historyQueries = 0;
  const runtime = new SessionRuntime({
    registerIpc: false,
    launchConfig: () => ({ workspace: '/fixture', trusted: true }),
    onCronRunRequested: async () => ({ sessionId: 'target-chat', summary: 'complete' }),
  });
  const client = new EventEmitter() as any;
  client.close = () => undefined;
  client.sendCommand = (command: ClientCommand) => {
    replies.push(command);
    if (command.type === 'cron_manage') {
      historyQueries++;
      const task = claimTask(nextGeneration ? 2 : 1);
      task.automation!.runs[0]!.status = !nextGeneration && historyQueries > 1 ? 'succeeded' : 'running';
      client.emit('event', { type: 'cron_result', request_id: command.request_id, jobs: [task] });
    }
  };
  const internal = runtime as any;
  internal.activeWorkspace = '/fixture';
  internal.activeWorkspaceTrusted = true;
  internal.client = client;
  internal.wireClient(client, internal.generation);
  t.after(() => runtime.dispose());
  const token = 'lingxi-cron-claim-v1:' + JSON.stringify(['occurrence', 1]);
  client.emit('event', { type: 'cron_run_requested', run_id: token, task: claimTask(1) });
  for (let attempt = 0; attempt < 30 && internal.activeCronExecutions > 0; attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  assert.equal(internal.activeCronExecutions, 0, 'the claim acknowledgement must release its controller lease');
  assert.equal((replies[0] as any).run_id, token, 'wire completion keeps the per-claim correlation token');
  return historyQueries;
}

test('claim completion waits for canonical SDK history to settle, preserving the wire token', async (t) => {
  assert.equal(await controller(t), 2, 'running canonical history requires a second acknowledgement query');
});

test('a late completed claim does not wait for a newer running claim of the same occurrence', async (t) => {
  assert.equal(await controller(t, true), 1, 'generation 1 must not hold its controller lease for generation 2');
});
