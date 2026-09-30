import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

test('native credential transactions prevent deleted values from being recached', {
  skip: process.platform !== 'darwin',
  timeout: 60_000,
}, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-fake-credential-store-'));
  const nativeRoot = fileURLToPath(new URL('../native/credential-broker/', import.meta.url));
  const fixture = fileURLToPath(new URL('./fixtures/credential-store.swift', import.meta.url));
  const binary = join(root, 'credential-store-regression');
  try {
    // Compile the complete production source with the test entrypoint and fake
    // SecItem implementations. The only disabled code is the XPC app's @main.
    execFileSync('xcrun', ['swiftc', '-swift-version', '5', '-parse-as-library',
      '-D', 'CREDENTIAL_BROKER_TESTING', '-module-cache-path', join(root, 'module-cache'),
      join(nativeRoot, 'BrokerCommon.swift'), join(nativeRoot, 'CredentialBrokerMain.swift'),
      fixture, '-o', binary,
    ], { encoding: 'utf8', timeout: 45_000 });
    const output = execFileSync(binary, [], { encoding: 'utf8', timeout: 10_000 });
    assert.match(output, /serialized read\/delete and cache replacement checks passed/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
