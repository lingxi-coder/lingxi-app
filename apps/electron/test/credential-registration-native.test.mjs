import assert from 'node:assert/strict';
import { test } from 'node:test';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

test('native broker registration recovers a matching plist after bootstrap failure', {
  skip: process.platform !== 'darwin',
  timeout: 60_000,
}, () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-fake-broker-registration-'));
  const nativeRoot = fileURLToPath(new URL('../native/credential-broker/', import.meta.url));
  const fixture = fileURLToPath(new URL('./fixtures/credential-registration.swift', import.meta.url));
  const binary = join(root, 'credential-registration-regression');
  try {
    // Complete production source, with its @main disabled. The registration
    // function receives fake launchctl; all written files live under tmp.
    execFileSync('xcrun', ['swiftc', '-swift-version', '5', '-parse-as-library',
      '-D', 'CREDENTIAL_BROKER_TESTING', '-module-cache-path', join(root, 'module-cache'),
      join(nativeRoot, 'BrokerCommon.swift'), join(nativeRoot, 'CredentialClientMain.swift'),
      fixture, '-o', binary,
    ], { encoding: 'utf8', timeout: 45_000 });
    const output = execFileSync(binary, [join(root, 'LaunchAgents')], {
      encoding: 'utf8', timeout: 10_000,
    });
    assert.match(output, /registration retry, login recovery, configuration replacement and healthy reuse checks passed/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
