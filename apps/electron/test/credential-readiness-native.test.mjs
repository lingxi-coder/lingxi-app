import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

test('native broker readiness gates operations without replaying mutations', {
  skip: process.platform !== 'darwin',
  timeout: 60_000,
}, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-fake-broker-registration-'));
  const nativeRoot = fileURLToPath(new URL('../native/credential-broker/', import.meta.url));
  const fixture = fileURLToPath(new URL('./fixtures/credential-readiness.swift', import.meta.url));
  const binary = join(root, 'credential-registration-regression');
  try {
    // Compile the production client with only its entry point disabled.
    // A real anonymous XPC endpoint delays readiness beyond the former 15s limit.
    execFileSync('xcrun', ['swiftc', '-swift-version', '5', '-parse-as-library',
      '-D', 'CREDENTIAL_BROKER_TESTING', '-module-cache-path', join(root, 'module-cache'),
      join(nativeRoot, 'BrokerCommon.swift'), join(nativeRoot, 'CredentialClientMain.swift'),
      fixture, '-o', binary,
    ], { encoding: 'utf8', timeout: 45_000 });
    const output = execFileSync(binary, [join(root, 'LaunchAgents')], {
      encoding: 'utf8', timeout: 25_000,
    });
    assert.match(output, /readiness gates operations and never replays mutations/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
