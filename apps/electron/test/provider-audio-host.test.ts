import type { ProviderAccountLaunch } from '../src/main/provider-accounts';
import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import type { spawn } from 'node:child_process';
import { ProviderAudioHost } from '../src/main/audio/providerAudioHost';
import { audioConfigurationDefaults } from '../src/shared/generatedAudioConfiguration';

// Every process in this file is mocked; binary discovery must not depend on a
// developer having built an unrelated workspace checkout.
const originalServerBin = process.env['LINGXI_BRIDGE_SERVER_BIN'];
process.env['LINGXI_BRIDGE_SERVER_BIN'] = new URL('./fixtures/provider-audio-host-mock-binary', import.meta.url).pathname;
after(() => {
  if (originalServerBin === undefined) delete process.env['LINGXI_BRIDGE_SERVER_BIN'];
  else process.env['LINGXI_BRIDGE_SERVER_BIN'] = originalServerBin;
});

function host(respond: (request: any) => unknown, accounts?: ProviderAccountLaunch) {
  const requests: any[] = [];
  const credentials: string[] = [];
  const service = new ProviderAudioHost({
    resolveProviderAccounts: async () => accounts ?? { providerAccountHome: '/mock/home', providerAccounts: { storeScope: 'mock', configRevision: 0, accounts: [], health: [] }, providerAccountCredentials: {} },
    isPackaged: false, resourcesPath: '/unused', cwd: () => '/tmp',
    session: () => ({ sessionId: 'session', profileId: 'named-profile', accountScope: 'account' }),
    spawnProcess: ((bin, args, opts) => {
      assert.deepEqual(args, ['--audio-service-json']);
      assert.equal(opts.cwd, '/tmp');
      assert.ok(!String(bin).includes('secret'));
      const child = new EventEmitter() as any;
      child.stdout = new PassThrough(); child.stderr = new PassThrough(); child.stdin = new PassThrough(); child.kill = () => child.emit('close', 0);
      child.stdin.on('data', (bytes: Buffer) => {
        const request = JSON.parse(bytes.toString());
        assert.ok(Object.keys(request).every((key) => ['profilesJson', 'credentials', 'region', 'kind', 'request', 'audioBase64', 'mimeType', 'text'].includes(key)), 'must match strict Rust envelope');
        requests.push(request);
        queueMicrotask(() => { child.stdout.write(JSON.stringify(respond(request))); child.emit('close', 0); });
      });
      return child;
    }) as typeof spawn,
  });
  return { service, requests, credentials };
}
const capability = { supported: true, readiness: 'ready', profileId: 'named-profile', providerId: 'openai', credentialId: 'vault-key', modelId: 'tts', models: [{ id: 'tts', voices: [] }], streaming: false, realtime: false };

test('provider audio uses exact session profile and sends credentials only on host stdin', async () => {
  const { service, requests, credentials } = host((request) => request.kind === 'capabilities' ? capability : { pcmBase64: 'AAAA', sampleRateHz: 24_000, usage: {} });
  const config = audioConfigurationDefaults(); config.speech.source = 'provider';
  const result = await service.execute('speech', config, { text: 'hello' }, { sessionId: 'session', profileId: 'named-profile', accountScope: 'account' }, new AbortController().signal);
  assert.equal(result.type, 'synthesized');
  assert.deepEqual(credentials, []);
  assert.equal(requests[0].request.session.profileId, 'named-profile');
  assert.equal('providerKeys' in requests[0], false);
  assert.equal('providerKeys' in requests[1], false);
  assert.equal(requests[1].request.cloud.binding, 'follow_session');
});

test('unsupported provider operation fails before credentials and dispatch without local fallback', async () => {
  const { service, requests, credentials } = host(() => ({ ...capability, supported: false, readiness: 'unsupported' }));
  const config = audioConfigurationDefaults(); config.recognition.source = 'provider';
  const result = await service.execute('recognition', config, { audioBase64: 'AAAA', mimeType: 'audio/wav' }, undefined, new AbortController().signal);
  assert.equal(result.type, 'failed'); assert.equal(requests.length, 1); assert.deepEqual(credentials, []);
});

test('a provider voice from another profile fails before provider dispatch', async () => {
  const { service, requests } = host(() => capability);
  const config = audioConfigurationDefaults(); config.speech.source = 'provider'; config.speech.voice = { source: 'provider', id: 'alloy', profileId: 'another-profile', modelId: 'tts' };
  const result = await service.execute('speech', config, { text: 'hello' }, undefined, new AbortController().signal);
  assert.equal(result.type, 'failed'); if (result.type === 'failed') assert.equal(result.error.kind, 'invalid_request');
  assert.equal(requests.length, 1);
});

test('credential presence alone does not turn a rejected capability into ready', async () => {
  const { service } = host(() => ({ ...capability, readiness: 'needs_configuration' }));
  const snapshot = await service.capabilities(audioConfigurationDefaults());
  assert.ok(snapshot.providerCapabilities?.every((entry) => entry.readiness === 'configurationRequired'));
  assert.equal(snapshot.realtimeReadiness?.ready, false);
});

test('usage and actual route preserve the provider-resolved model and account instead of catalog guesses', async () => {
  const context = { operationId: 'provider-operation', profileId: 'named-profile', providerId: 'openai', modelId: 'actual-audio-model', accountScope: 'credential-account' };
  const { service } = host((request) => request.kind === 'capabilities' ? capability : { pcmBase64: 'AAA=', sampleRateHz: 24_000, usage: { characters: 5 }, usageContext: context });
  const config = audioConfigurationDefaults(); config.speech.source = 'provider';
  const usage: unknown[] = []; const routes: unknown[] = [];
  const result = await service.execute('speech', config, { text: 'hello', onUsage: (value, route) => usage.push({ value, route }), onResolvedRoute: (route) => routes.push(route) }, undefined, new AbortController().signal);
  assert.equal(result.type, 'synthesized'); assert.deepEqual(routes, [context]); assert.deepEqual(usage, [{ value: { characters: 5 }, route: context }]);
});

test('a ready native endpoint keeps a null model and profile-scoped voice without inventing an ID', async () => {
  const nativeCapability = { ...capability, modelId: null, models: [{ id: null, voices: [{ id: 'native-voice', label: 'Native voice' }] }] };
  const { service, requests } = host((request) => request.kind === 'capabilities' ? nativeCapability : { pcmBase64: 'AAA=', sampleRateHz: 24_000, usage: { characters: 1 }, usageContext: { operationId: 'native-operation', profileId: 'named-profile', providerId: 'xai', modelId: '', accountScope: 'native-account' } });
  const config = audioConfigurationDefaults(); config.speech.source = 'provider'; config.speech.voice = { source: 'provider', profileId: 'named-profile', modelId: null, id: 'native-voice' };
  const routes: unknown[] = [];
  const result = await service.execute('speech', config, { text: 'a', onResolvedRoute: (route) => routes.push(route) }, undefined, new AbortController().signal);
  assert.equal(result.type, 'synthesized'); assert.equal(requests[1].request.cloud.modelId, null); assert.equal(requests[1].request.voice, 'native-voice');
  assert.equal((routes[0] as { modelId: unknown }).modelId, null);
  const snapshot = await service.capabilities(config);
  assert.deepEqual(snapshot.providerCatalog?.find((entry) => entry.kind === 'speech')?.models, nativeCapability.models);
});


test('audio forwards the canonical account envelope and contains only account authority', async () => {
  const id = '11111111-1111-4111-8111-111111111111';
  const accounts: ProviderAccountLaunch = { providerAccountHome: '/canonical/host', providerAccounts: { storeScope: 'broker-channel', configRevision: 1, health: [], accounts: [
    { accountId: id, providerId: 'openai', executionBindingId: 'openai', authKind: 'api_key', displayName: 'A', enabled: true, userPriority: 0, configRevision: 1, identityGeneration: 1,
      credentialRef: { storeScope: 'broker-channel', secretRef: `acct.${id}` } }] }, providerAccountCredentials: { [id]: { credential_revision: 1, identity_generation: 1, kind: { kind: 'api_key', key: 'account-only-secret' } } } };
  const { service, requests } = host(request => request.kind === 'capabilities' ? { ...capability, profileId: 'openai', credentialId: 'openai' } : { pcmBase64: 'AAAA', sampleRateHz: 24_000 }, accounts);
  const config = audioConfigurationDefaults(); config.speech.source = 'provider';
  const result = await service.execute('speech', config, { text: 'hello' }, { sessionId: 'session', profileId: 'openai', accountScope: `account:${id}` }, new AbortController().signal);
  assert.equal(result.type, 'synthesized');
  assert.ok(requests.length >= 2);
  for (const request of requests) {
    assert.equal('providerKeys' in request, false);
    assert.equal(request.credentials.provider_account_home, '/canonical/host');
    assert.equal(request.credentials.provider_account_credentials[id].kind.key, 'account-only-secret');
  }
});
