import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  isCurrentCredentialTransaction,
  persistProviderCredentialAndApplyModel,
  persistProviderCredentialInput,
} from '../src/renderer/bridge/providerCredentials';

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

test('closing Settings invalidates late credential transaction updates without cancelling host work', () => {
  let mounted = true;
  let generation = 4;
  assert.equal(isCurrentCredentialTransaction(mounted, generation, generation), true);

  // Close/Escape marks the component stale and advances its generation. A
  // persistence or restart result that arrives afterwards must not update or
  // apply anything in the unmounted Settings instance.
  mounted = false;
  generation += 1;
  assert.equal(isCurrentCredentialTransaction(mounted, 4, generation), false);
  assert.equal(isCurrentCredentialTransaction(true, 4, generation), false);
});

test('credential persistence clears the secret before applying a deferred model without restarting', async () => {
  const events: string[] = [];
  await persistProviderCredentialAndApplyModel(
    'deepseek',
    'sk-test-secret',
    async () => { events.push('persist'); },
    () => { events.push('clear'); },
    'deepseek/deepseek-v4-flash',
    async (reference) => { events.push(`apply:${reference}`); },
  );
  assert.deepEqual(events, ['persist', 'clear', 'apply:deepseek/deepseek-v4-flash']);
});

test('credential persistence does not clear or apply when the host rejects the write', async () => {
  const events: string[] = [];
  await assert.rejects(
    persistProviderCredentialAndApplyModel(
      'deepseek',
      'sk-test-secret',
      async () => { events.push('persist'); throw new Error('host unavailable'); },
      () => { events.push('clear'); },
      'deepseek/deepseek-v4-flash',
      async () => { events.push('apply'); },
    ),
    /host unavailable/,
  );
  assert.deepEqual(events, ['persist']);
});

test('model application failure happens after the persisted secret leaves renderer state', async () => {
  const events: string[] = [];
  await assert.rejects(
    persistProviderCredentialAndApplyModel(
      'deepseek',
      'sk-test-secret',
      async () => { events.push('persist'); },
      () => { events.push('clear'); },
      'deepseek/deepseek-v4-flash',
      async () => { events.push('apply'); throw new Error('model switch failed'); },
    ),
    /model switch failed/,
  );
  assert.deepEqual(events, ['persist', 'clear', 'apply']);
});
