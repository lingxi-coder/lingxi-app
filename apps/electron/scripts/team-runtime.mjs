import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const productRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
export const TEAM_BINARIES = ['harness-team-host', 'harness-team-worker'];
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const output = () => resolve(productRoot, process.env.CARGO_TARGET_DIR || 'target');
const provenancePath = () => join(output(), 'team-runtime-provenance.json');
function pinnedRevision() {
  const manifest = readFileSync(join(productRoot, 'Cargo.toml'), 'utf8');
  const entry = /^harness-runtime\s*=\s*\{([^\n]+)\}/mu.exec(manifest)?.[1];
  const revision = /rev\s*=\s*"([a-f0-9]{40})"/u.exec(entry ?? '')?.[1];
  if (!revision || !entry.includes('https://github.com/lingxi-coder/harness-runtime.git')) throw new Error('Missing immutable Harness pin');
  return revision;
}
function buildInputs() {
  const files = ['Cargo.toml', 'Cargo.lock', 'apps/bridge-server/Cargo.toml', 'apps/bridge-server/src/bin/team_host.rs', 'apps/bridge-server/src/bin/team_worker.rs'];
  const input = createHash('sha256');
  for (const file of files) input.update(file).update('\0').update(readFileSync(join(productRoot, file))).update('\0');
  return input.digest('hex');
}
export function resolvePackagedTeamBinaries() {
  const provenance = JSON.parse(readFileSync(provenancePath(), 'utf8'));
  if (provenance.version !== 2 || provenance.inputs !== buildInputs() || provenance.revision !== pinnedRevision()
    || (provenance.development && process.env.LINGXI_TEAM_RUNTIME_VALIDATION !== '1')) throw new Error('Team binary provenance does not match the product Harness pin');
  const binaries = TEAM_BINARIES.map(name => join(output(), 'release', name));
  for (const [index, binary] of binaries.entries()) if (!existsSync(binary) || provenance.sha256[TEAM_BINARIES[index]] !== hash(binary)) throw new Error('Team binary changed after its pinned build');
  return binaries;
}
export function buildTeamRuntime() {
  const source = JSON.parse(execFileSync(process.env.PYTHON || 'python3', [join(productRoot, 'scripts/lib/runtime_source.py'), '--json'], {
    cwd: productRoot, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024,
  }));
  if (source.revision !== pinnedRevision() || (source.development && process.env.LINGXI_TEAM_RUNTIME_VALIDATION !== '1')) throw new Error('Team runtime requires the authoritative pinned Harness source');
  // The product bridge and both private executables resolve the same immutable
  // Git SDK and share one target directory, feature graph and portable flags.
  const binaries = process.env.LINGXI_BRIDGE_SERVER_BIN ? TEAM_BINARIES : ['bridge-server', ...TEAM_BINARIES];
  execFileSync('cargo', ['build', '--locked', '--release', '-p', 'bridge-server', ...binaries.flatMap(name => ['--bin', name])], {
    cwd: productRoot, stdio: 'inherit', env: { ...process.env, CARGO_TARGET_DIR: output() },
  });
  writeFileSync(provenancePath(), JSON.stringify({ version: 2, inputs: buildInputs(), revision: source.revision, development: source.development,
    sha256: Object.fromEntries(TEAM_BINARIES.map(name => [name, hash(join(output(), 'release', name))])) }, null, 2));
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) buildTeamRuntime();
