import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalAppCoreClient } from '../src/main/local-apps/coreClient.ts';
import { CoreLocalAppStore } from '../src/main/local-apps/coreStore.ts';

const fixturePath = process.env.LINGXI_DESKTOP_AUTHORING_FIXTURE;

test('Desktop decodes a native private-stdio authoring fixture', { skip: !fixturePath }, async t => {
  const expectedHash = process.env.LINGXI_DESKTOP_AUTHORING_FIXTURE_SHA256;
  assert.match(expectedHash ?? '', /^[a-f0-9]{64}$/, 'a source fixture SHA-256 is required');
  const bytes = await fs.readFile(fixturePath!);
  assert.equal(createHash('sha256').update(bytes).digest('hex'), expectedHash);
  const fixture = JSON.parse(bytes.toString('utf8'));
  assert.equal(fixture.schema, 1);

  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-native-wire-'));
  const executable = join(root, 'fixture-service.cjs');
  // Replay the exact Rust-produced response values while retaining the real
  // Desktop client's correlated stdio envelope and validating each request.
  await fs.writeFile(executable, `#!/usr/bin/env node
const fs = require('node:fs');
const readline = require('node:readline');
const { isDeepStrictEqual } = require('node:util');
const fixture = JSON.parse(fs.readFileSync(process.env.LINGXI_DESKTOP_AUTHORING_FIXTURE, 'utf8'));
const steps = ['ensure', 'register', 'get', 'fingerprint', 'publish', 'getPublished', 'publication', 'replay'];
let index = 0;
readline.createInterface({ input: process.stdin }).on('line', line => {
  const input = JSON.parse(line);
  const name = steps[index++];
  if (!name || !isDeepStrictEqual(input.request, fixture[name].request.request)) {
    process.stdout.write(JSON.stringify({ id: input.id, ok: false, error: { message: 'Unexpected native fixture request: ' + name } }) + '\\n');
    return;
  }
  process.stdout.write(JSON.stringify({ ...fixture[name].response, id: input.id }) + '\\n');
});
`, { mode: 0o700 });
  const client = new LocalAppCoreClient(executable, root);
  t.after(async () => { await client.dispose(); await fs.rm(root, { recursive: true, force: true }); });
  const store = new CoreLocalAppStore(join(root, 'local-apps', 'v2'), client);
  const ensure = fixture.ensure.request.request;
  const proof = await store.ensureAuthoringSource(ensure.sourceDirectory, ensure.projectRef, ensure.requestId);
  assert.deepEqual(proof, fixture.ensure.response.value);
  assert.equal(proof.requestId, ensure.requestId);
  const registration = fixture.register.request.request;
  const authoring = await store.registerAuthoring(registration.sourceId, registration.requestId, registration.authoring, registration.setCurrent);
  assert.deepEqual(authoring, fixture.register.response.value);
  assert.equal(authoring.sessionMode, 'code');
  const get = fixture.get.request.request;
  const grant = await store.getAuthoringGrant(get.sourceDirectory, get.projectRef, get.authoringSessionId);
  assert.deepEqual(grant, fixture.get.response.value);
  assert.equal(grant.sourceId, proof.sourceId);
  assert.equal(grant.sessionMode, 'code');
  assert.equal(grant.authoring.authoringSessionId, registration.authoring.authoringSessionId);
  assert.deepEqual(grant.authoring.projectRef, registration.authoring.projectRef);

  for (const step of ['fingerprint', 'publish', 'getPublished', 'publication', 'replay']) {
    assert.deepEqual(await client.request(fixture[step].request.request), fixture[step].response.value);
  }
  const result = fixture.publish.response.value;
  assert.deepEqual(fixture.replay.response.value, result);
  assert.equal(fixture.getPublished.response.value.ownerAppId, result.record.appId);
  assert.equal(fixture.publication.response.value.sourceId, proof.sourceId);
  assert.equal(fixture.publication.response.value.publisher.authoringSessionId, registration.authoring.authoringSessionId);
});
