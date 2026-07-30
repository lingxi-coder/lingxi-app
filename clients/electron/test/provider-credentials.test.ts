import { test } from 'node:test';
import assert from 'node:assert/strict';

import { persistProviderCredentialInput } from '../src/renderer/bridge/providerCredentials';

test('provider credential input is cleared only after persistence succeeds', async () => {
  const writes: Array<[string, string]> = [];

  const saved = await persistProviderCredentialInput(
    'deepseek',
    '  sk-test-secret  ',
    async (providerId, credential) => { writes.push([providerId, credential]); },
  );

  assert.deepEqual(writes, [['deepseek', 'sk-test-secret']]);
  assert.equal(saved, true);
});

test('provider credential input remains available when persistence fails', async () => {
  let saved = false;

  await assert.rejects(
    persistProviderCredentialInput(
      'deepseek',
      'sk-test-secret',
      async () => { throw new Error('bridge client not connected'); },
    ).then((result) => { saved = result; }),
    /bridge client not connected/,
  );

  assert.equal(saved, false);
});
