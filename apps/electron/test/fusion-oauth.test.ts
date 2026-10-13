import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { SessionRuntime } from '../src/main/bridge.js';

const session = { access_token: 'test-private-token', refresh_token: 'test-private-refresh', expires_at: 123, fedramp: false };
const command = { type: 'run_slash_command', raw: '/fusion compare approaches' };
function fixture(resolveOpenAiOAuth = async () => session as typeof session | undefined) {
  const calls: string[] = [];
  const runtime = new SessionRuntime({
    registerIpc: false,
    isPackaged: true,
    launchConfig: () => ({ workspace: '/workspace', trusted: true }),
    resolveOpenAiOAuth,
    resolveProviderCredential: async id => { calls.push(`key:${id}`); return undefined; },
    onFirstPromptSent: () => { calls.push('commit'); },
  });
  const state = runtime as any;
  const client = Object.assign(new EventEmitter(), {
    sendCommand: (value: any) => {
      if (value.type === 'set_provider_credential') {
        assert.equal(JSON.parse(value.credential).refresh_token, undefined, 'an engine never receives the refresh token');
        calls.push(`serve:${value.provider_id}`);
        queueMicrotask(() => state.handleProviderCredentialStatus({
          type: 'provider_credential_status', operation_id: value.operation_id,
          configured_provider_ids: [value.provider_id], storage_encrypted: false,
        }));
      } else calls.push(`send:${value.type}`);
    },
    cancel: () => calls.push('cancel'),
  });
  state.activeWorkspace = '/workspace';
  state.activeWorkspaceTrusted = true;
  state.selectedModelReference = 'deepseek/deepseek-flash';
  state.client = client;
  state.state = { status: 'connected' };
  state.credentialRoutingSettings = { fusion: { panelModels: [{ profile: 'openai-chatgpt', model: 'gpt-5.6-sol' }] } };
  runtime.restart = async () => { calls.push('restart'); };
  return { runtime, state, calls };
}

test('Fusion serves the Codex access token in place without restarting or changing the primary model', async () => {
  const { runtime, state, calls } = fixture();
  await runtime.dispatchCommand(command);
  assert.deepEqual(calls, ['serve:openai-chatgpt', 'send:run_slash_command', 'commit']);
  assert.equal(state.selectedModelReference, 'deepseek/deepseek-flash');
  assert.equal(runtime.turnActive, false);
  await runtime.dispatchCommand(command);
  assert.equal(calls.filter(value => value === 'serve:openai-chatgpt').length, 1, 'a token already served is not sent again');
});

test('Fusion reports missing OAuth without sending or committing a command', async () => {
  const { runtime, calls } = fixture(async () => undefined);
  await assert.rejects(runtime.dispatchCommand(command), /authentication is unavailable/);
  assert.deepEqual(calls, []);
});

test('Fusion without a Codex panel never loads the Codex session', async () => {
  const { runtime, state, calls } = fixture(async () => { throw new Error('must not load'); });
  state.credentialRoutingSettings = { fusion: { panelModels: [{ profile: 'deepseek', model: 'deepseek-flash' }] } };
  await runtime.dispatchCommand(command);
  assert.deepEqual(calls.filter(value => !value.startsWith('key:')), ['send:run_slash_command', 'commit']);
});

test('Fusion setup and publication retry bypass OAuth activation and commit their persisted messages', async () => {
  const { runtime, calls } = fixture(async () => { throw new Error('must not load'); });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion setup' });
  await runtime.dispatchCommand({ type: 'run_slash_command', raw: '/fusion --retry-publication saved' });
  assert.deepEqual(calls, ['send:run_slash_command', 'commit', 'send:run_slash_command']);
});
