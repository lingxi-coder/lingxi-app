import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { syncBuiltinESMExports } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import test from 'node:test';

test('product builds share one locked graph and reject stale Team binary provenance', async context => {
  const product = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
  const fixture = realpathSync(mkdtempSync(join(tmpdir(), 'team-runtime-build-')));
  context.after(() => rmSync(fixture, { recursive: true, force: true }));
  for (const file of ['Cargo.toml', 'Cargo.lock', 'apps/bridge-server/Cargo.toml', 'apps/bridge-server/src/bin/team_host.rs', 'apps/bridge-server/src/bin/team_worker.rs', 'apps/electron/scripts/team-runtime.mjs']) {
    mkdirSync(dirname(join(fixture, file)), { recursive: true });
    copyFileSync(join(product, file), join(fixture, file));
  }
  const revision = /harness-runtime\s*=\s*\{[^\n]*rev\s*=\s*"([a-f0-9]{40})"/u.exec(readFileSync(join(fixture, 'Cargo.toml'), 'utf8'))[1];
  const variables = ['CARGO_TARGET_DIR', 'LINGXI_BRIDGE_SERVER_BIN', 'LINGXI_TEAM_RUNTIME_VALIDATION', 'PYTHON'];
  const previous = Object.fromEntries(variables.map(name => [name, process.env[name]]));
  context.after(() => { for (const name of variables) { if (previous[name] === undefined) delete process.env[name]; else process.env[name] = previous[name]; } });
  const output = join(fixture, 'isolated-target');
  process.env.CARGO_TARGET_DIR = output;
  delete process.env.LINGXI_BRIDGE_SERVER_BIN;
  delete process.env.LINGXI_TEAM_RUNTIME_VALIDATION;
  let source = { revision, development: false };
  const builds = [];
  context.mock.method(childProcess, 'execFileSync', (file, args, options) => {
    assert.equal(options.cwd, fixture);
    if (file !== 'cargo') return JSON.stringify(source);
    builds.push({ args, target: options.env.CARGO_TARGET_DIR });
    mkdirSync(join(output, 'release'), { recursive: true });
    for (const name of ['bridge-server', 'harness-team-host', 'harness-team-worker']) writeFileSync(join(output, 'release', name), `synthetic ${name}`);
  });
  syncBuiltinESMExports();
  context.after(() => { context.mock.restoreAll(); syncBuiltinESMExports(); });
  const runtime = await import(pathToFileURL(join(fixture, 'apps/electron/scripts/team-runtime.mjs')).href);
  runtime.buildTeamRuntime();
  assert.deepEqual(builds, [{ args: ['build', '--locked', '--release', '-p', 'bridge-server', '--bin', 'bridge-server', '--bin', 'harness-team-host', '--bin', 'harness-team-worker'], target: output }]);
  assert.equal(runtime.resolvePackagedTeamBinaries().length, 2);
  writeFileSync(join(output, 'release/harness-team-host'), 'tampered');
  assert.throws(runtime.resolvePackagedTeamBinaries, /changed/u);
  runtime.buildTeamRuntime();
  const manifest = join(fixture, 'Cargo.toml'), original = readFileSync(manifest);
  writeFileSync(manifest, `${original}\n# graph changed\n`);
  assert.throws(runtime.resolvePackagedTeamBinaries, /provenance/u);
  writeFileSync(manifest, original);
  assert.equal(runtime.resolvePackagedTeamBinaries().length, 2);
  source = { revision: '0'.repeat(40), development: false };
  assert.throws(runtime.buildTeamRuntime, /authoritative/u);
  source = { revision, development: true };
  assert.throws(runtime.buildTeamRuntime, /authoritative/u);
  process.env.LINGXI_TEAM_RUNTIME_VALIDATION = '1';
  runtime.buildTeamRuntime();
  delete process.env.LINGXI_TEAM_RUNTIME_VALIDATION;
  assert.throws(runtime.resolvePackagedTeamBinaries, /provenance/u);
});
