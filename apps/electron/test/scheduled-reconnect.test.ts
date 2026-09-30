import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import type { ClientCommand, CronJobDto } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';
import { ScheduledTaskService } from '../src/main/scheduled.js';

const workspace = '/fixture';
const sourceId = '11111111-2222-4333-8444-555555555555';
const token = 'lingxi-cron-claim-v1:' + JSON.stringify(['occurrence', 1]);

function transport() {
  const client = new EventEmitter() as EventEmitter & { sendCommand(command: ClientCommand): void; close(): void };
  client.close = () => undefined;
  client.sendCommand = () => undefined;
  return client;
}

async function until(predicate: () => boolean) {
  for (let attempt = 0; attempt < 50 && !predicate(); attempt++) await new Promise((resolve) => setImmediate(resolve));
  assert.equal(predicate(), true, 'the expected runtime dispatch must occur');
}

for (const cancelledBeforeBinding of [false, true]) {
test(cancelledBeforeBinding
  ? 'an unbound cancellation recovery replay returns the cached failure after controller disconnect without another execution'
  : 'an admitted claim replays after controller disconnect and completes once on the new transport', async (t) => {
  const job = {
    id: 'job', prompt: 'Scheduled task', automation: {
      model: 'provider/model', reasoning: { type: 'automatic' }, runMode: 'new_session', notificationPolicy: 'none',
      runs: [{ id: 'occurrence', claimGeneration: 1, status: 'running', scheduledAt: 1 }],
    },
  } as unknown as CronJobDto;
  let executions = 0;
  let target: SessionRuntime | undefined;
  let targetClient: ReturnType<typeof transport> | undefined;
  let service!: ScheduledTaskService;
  const source = new SessionRuntime({
    registerIpc: false, sessionId: sourceId, projectPath: workspace,
    launchConfig: () => ({ workspace, trusted: true }),
    onCronRunRequested: (runtime, event) => service.run(runtime, event.run_id, event.task),
  });
  const bindTransport = (client: ReturnType<typeof transport>) => {
    const internal = source as any;
    internal.activeWorkspace = workspace;
    internal.activeWorkspaceTrusted = true;
    internal.client = client;
    internal.wireClient(client, internal.generation);
    internal.setState({ status: 'connected' });
  };
  const settings = {
    getPublic: () => ({ projects: [workspace] }),
    isTrustedWorkspace: () => true,
    isSessionArchived: () => false,
  };
  const manager = {
    withBackgroundSession: async (ref: { sessionId: string }, _empty: boolean, _model: unknown, operation: (runtime: SessionRuntime) => Promise<unknown>) => {
      executions++;
      target = new SessionRuntime({
        registerIpc: false, sessionId: ref.sessionId, projectPath: workspace,
        launchConfig: () => ({ workspace, trusted: true }),
      });
      targetClient = transport();
      const internal = target as any;
      internal.activeWorkspace = workspace;
      internal.activeWorkspaceTrusted = true;
      internal.client = targetClient;
      internal.credentialRoutingSettings = {};
      internal.ensureModelProviderCredential = async () => undefined;
      internal.scheduledModelCatalog = async () => ({
        details: [{ reference: 'provider/model', reasoning: { options: [], provider_default: { type: 'automatic' } } }],
      });
      internal.wireClient(targetClient, internal.generation);
      const commands: ClientCommand[] = [];
      targetClient.sendCommand = (command) => commands.push(command);
      (targetClient as any).commands = commands;
      return operation(target);
    },
  };
  service = new ScheduledTaskService(settings as any, manager as any, {} as any, () => false, () => undefined);
  t.after(async () => { service.dispose(); await source.dispose(); await target?.dispose(); });
  const first = transport();
  const firstReplies: ClientCommand[] = [];
  first.sendCommand = (command) => {
    firstReplies.push(command);
    if (command.type === 'cron_run_started' && !cancelledBeforeBinding) {
      job.automation!.runs[0]!.sessionId = command.session_id;
      job.automation!.runs[0]!.startedAt = 2;
      first.emit('event', { type: 'cron_run_bound', run_id: command.run_id });
    }
  };
  bindTransport(first);
  first.emit('event', { type: 'cron_run_requested', run_id: token, task: structuredClone(job) });
  await until(() => cancelledBeforeBinding
    ? firstReplies.some((command) => command.type === 'cron_run_started')
    : (targetClient as any)?.commands.some((command: ClientCommand) => command.type === 'scheduled_run_turn'));
  assert.equal(executions, 1);
  if (cancelledBeforeBinding) {
    job.automation!.runs[0]!.error = 'lingxi-host-cancel-requested-v1';
    assert.equal(job.automation!.runs[0]!.sessionId, undefined);
    assert.equal(job.automation!.runs[0]!.startedAt, undefined);
  }
  first.emit('close', 1006, 'controller disconnected');
  (source as any).generation++;
  if (cancelledBeforeBinding) (source as any).clearPendingConnectionOperations();
  const second = transport();
  const secondReplies: ClientCommand[] = [];
  second.sendCommand = (command) => {
    secondReplies.push(command);
    if (command.type === 'cron_manage') {
      const settled = structuredClone(job);
      settled.automation!.runs[0]!.status = cancelledBeforeBinding ? 'cancelled' : 'succeeded';
      second.emit('event', { type: 'cron_result', request_id: command.request_id, jobs: [settled] });
    }
  };
  bindTransport(second);
  // The producer retains binding or cancellation recovery metadata under the same claim token.
  second.emit('event', { type: 'cron_run_requested', run_id: token, task: structuredClone(job) });
  if (!cancelledBeforeBinding) targetClient!.emit('event', { type: 'scheduled_run_finished', run_id: token, summary: 'completed once' });
  await until(() => (source as any).activeCronExecutions === 0);
  assert.equal(executions, 1, 'recovery redelivery must reuse the existing target admission');
  if (cancelledBeforeBinding) {
    assert.equal((targetClient as any).commands.filter((command: ClientCommand) => command.type === 'scheduled_run_turn').length, 0);
    assert.equal(secondReplies.filter((command) => command.type === 'cron_run_started').length, 0);
  }
  assert.equal(firstReplies.filter((command) => command.type === 'cron_run_completed').length, 0);
  const completions = secondReplies.filter((command) => command.type === 'cron_run_completed');
  assert.equal(completions.length, 1);
  assert.deepEqual(completions[0], cancelledBeforeBinding ? {
    type: 'cron_run_completed', run_id: token, error: 'interrupted: Scheduled connection closed.',
  } : {
    type: 'cron_run_completed', run_id: token, session_id: target!.sessionId, summary: 'completed once',
  });
});
}
