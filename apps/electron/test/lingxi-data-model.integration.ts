import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, readFile, realpath, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

import { TeamPrivateStore } from '../src/main/teams/privateStore.js';
import { runTeamHost, type BotChatScope, type TeamDataProof, type TeamDatasetOffer } from '../src/main/teams/process.js';
import type { ProviderAccountSnapshot } from '../src/shared/providerAccounts.js';

const [fixturePath] = process.argv.slice(2);
if (!fixturePath) throw new Error('Data model fixture required');
const { endpoint, ownerFile, folder, offer, proof, scope, desired } = JSON.parse(await readFile(fixturePath, 'utf8')) as {
  endpoint: string; ownerFile: string; folder: string; offer: TeamDatasetOffer; proof: TeamDataProof; scope: BotChatScope;
  desired: { action: 'put'; record: string }
    | { action: 'openReportReview'; release: string; partition: string; group: string }
    | { action: 'handoff'; reference: unknown; sharedBotId: string };
};
const modelEndpoint = process.env.LINGXI_TEST_MODEL_ENDPOINT;
const modelId = process.env.LINGXI_TEST_MODEL_ID;
const configuredHost = process.env.TEAM_V3_HOST_BIN;
const configuredWorker = process.env.TEAM_V3_WORKER_BIN;
assert.ok(modelEndpoint && modelId && configuredHost && configuredWorker);
assert.equal(new URL(modelEndpoint).hostname, '127.0.0.1', 'the fixture only sends data to a loopback model');
const [host, worker, token] = await Promise.all([realpath(configuredHost), realpath(configuredWorker), readFile(ownerFile, 'utf8')]);
const userData = join(folder, 'model-desktop', proof.taskId, desired.action);
const accountHome = join(userData, 'provider-home');
await mkdir(accountHome, { recursive: true, mode: 0o700 });
const privateStore = new TeamPrivateStore(userData);
const paths = await privateStore.owner('fixture-owner');

const profileName = 'fixture-openai-chat';
const accountId = randomUUID();
const storeScope = 'lingxi_data_model_fixture';
const providerAccounts: ProviderAccountSnapshot = {
  storeScope, configRevision: 1, health: [],
  accounts: [{ accountId, providerId: 'fixture', displayName: 'Loopback model', executionBindingId: profileName,
    authKind: 'api_key', credentialRef: { storeScope, secretRef: `acct.${accountId}` }, enabled: true,
    userPriority: 0, configRevision: 1, identityGeneration: 1 }],
};
const registry = join(accountHome, 'provider-accounts');
const versions = join(registry, 'versions', storeScope);
await mkdir(versions, { recursive: true, mode: 0o700 });
await writeFile(join(registry, storeScope + '.json'), JSON.stringify(providerAccounts), { mode: 0o600 });
await writeFile(join(versions, accountId + '.json'), JSON.stringify({ accountId, identityGeneration: 1, credentialRevision: 1 }), { mode: 0o600 });
const profile = { provider_id: 'fixture', profile_name: profileName, base_url: modelEndpoint, protocol: 'open_ai_chat', auth: 'api_key',
  credential: { source: 'host_managed', key: 'team-selected-model' },
  models: [{ display_model: modelId, request_model: modelId, billing_model: modelId,
    capability_support: { streaming: 'supported', tools: 'supported' } }], extra: {} };
const handoff = desired.action === 'handoff';
const tool = handoff ? 'TeamDataHandoff' : 'TeamDataWrite';
const message = { messageId: 'model-message', seq: 1, changeSeq: 1, changeId: 'model-change',
  author: { kind: 'user', uid: 'fixture-requester', displayName: 'Requester' }, createdAt: Date.now(),
  text: `请用 ${tool} 执行一次操作。字段和值如下。\n` + JSON.stringify(desired) + '\n/no_think' };
const inputBatchRef = await privateStore.dataRef('fixture-owner', { schemaVersion: 1, batchId: proof.taskId, ...scope,
  expectedSessionRevision: 0, fromSeqExclusive: 0, toSeqInclusive: 1, lastChangeSeq: 1, bootstrap: true,
  currentMessageId: message.messageId, messages: [message], changes: [] });

async function callProduct(body: Record<string, unknown>): Promise<Record<string, unknown>> {
  const response = await fetch(endpoint + '/v1/product-data', {
    method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify(body), signal: AbortSignal.timeout(20000),
  });
  const result = await response.json();
  assert.equal(response.status, 200, JSON.stringify(result));
  return result as Record<string, unknown>;
}

const effects: Record<string, unknown>[] = [];
let stepId: string | undefined;
const result = await runTeamHost({ executable: host, home: paths.home, tmp: paths.tmp,
  signal: AbortSignal.timeout(105000), assertCurrent: () => {},
  authorize: async () => {
    assert.deepEqual(await callProduct({ action: 'botDataOffers', ...proof }), { datasets: [offer] });
  },
  bootstrap: { version: 3, type: 'run', storageRoot: paths.runtime, workerExecutable: worker, scope, taskId: proof.taskId,
    invocationId: randomUUID(), knowledge: [], inputBatchRef, deadlineAt: Date.now() + 100000, sharedData: [offer],
    instructions: `Use ${tool} exactly once with the requested arguments. After the tool succeeds, reply with the word Done.`,
    model: { model_provider: profileName, model_protocol: 'open_ai_chat', model_id: modelId, model_endpoint: modelEndpoint, model_profile: profile },
    credentials: { provider_account_home: accountHome, provider_accounts: providerAccounts, provider_account_profiles: [profile],
      provider_account_credentials: { [accountId]: { credential_revision: 1, identity_generation: 1, kind: { kind: 'api_key', key: 'loopback' } } } },
    limits: { max_turns: 3, context_window_tokens: 8192, output_tokens_per_request: 768, physical_dispatches: 3 },
  },
  operate: async operation => {
    assert.equal(operation.tool, handoff ? 'data_handoff' : 'data_write');
    assert.deepEqual(operation.arguments, desired);
    stepId = createHash('sha256').update(operation.logicalStepId).digest('hex');
    const saved = await callProduct(desired.action === 'handoff'
      ? { action: 'botDataHandoff', ...proof, accessVersion: offer.version, stepId, reference: desired.reference, sharedBotId: desired.sharedBotId }
      : { action: 'botDataOperation', ...proof, accessVersion: offer.version, operation: operation.arguments, stepId });
    effects.push(saved);
    return { established: true, succeeded: true, result: saved };
  },
});
assert.equal(result.status, 'completed', JSON.stringify({ action: desired.action, status: result.status,
  effects: effects.length, dispatches: result.status !== 'task_checkpoint' && result.dispatchedRequests,
  deniedTools: result.status !== 'task_checkpoint' && result.deniedTools }));
if (desired.action === 'openReportReview') {
  assert.ok(effects.length >= 1);
  for (const retry of effects.slice(1)) assert.deepEqual(retry, effects[0]);
} else {
  assert.equal(effects.length, 1);
}
if (desired.action === 'put') {
  assert.deepEqual(effects[0], { dataset: offer.dataset, record: desired.record, schemaVersion: 1, revision: 1 });
} else if (desired.action === 'openReportReview') {
  const key = createHash('sha256').update(JSON.stringify([desired.release, desired.partition, desired.group, 'review'])).digest('hex');
  assert.deepEqual(effects[0], { review: key, state: 'open' });
} else {
  assert.equal(typeof effects[0]?.taskId, 'string');
  assert.deepEqual(effects[0]?.reference, desired.reference);
}
console.log(JSON.stringify({ model: modelId, tool, action: desired.action, calls: effects.length,
  task: proof.taskId, stepId, result: effects[0] }));
