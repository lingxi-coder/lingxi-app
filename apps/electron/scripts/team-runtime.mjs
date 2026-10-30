import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const productRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
export const TEAM_BINARIES = ['harness-team-host', 'harness-team-worker'];
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const output = () => resolve(productRoot, process.env.CARGO_TARGET_DIR || 'target', 'team-runtime');
const provenancePath = () => join(output(), 'team-runtime-provenance.json');
function pinnedRevision() {
  const manifest = readFileSync(join(productRoot, 'Cargo.toml'), 'utf8');
  const entry = /^harness-runtime\s*=\s*\{([^\n]+)\}/mu.exec(manifest)?.[1];
  const revision = /rev\s*=\s*"([a-f0-9]{40})"/u.exec(entry ?? '')?.[1];
  if (!revision || !entry.includes('https://github.com/lingxi-coder/harness-runtime.git')) throw new Error('Missing immutable Harness pin');
  return revision;
}
export function resolvePackagedTeamBinaries() {
  const provenance = JSON.parse(readFileSync(provenancePath(), 'utf8'));
  if (provenance.version !== 1 || provenance.revision !== pinnedRevision()
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
  const remap = `--remap-path-prefix=${source.root}=/harness-runtime --remap-path-prefix=${output()}=/team-target`;
  execFileSync('cargo', ['build', '--locked', '--release', '--manifest-path', join(source.root, 'Cargo.toml'),
    '-p', 'harness-runtime', '--no-default-features', '--features', 'desktop', ...TEAM_BINARIES.flatMap(name => ['--bin', name])], {
    cwd: source.root, stdio: 'inherit', env: { ...process.env, CARGO_TARGET_DIR: output(), RUSTFLAGS: `${process.env.RUSTFLAGS || ''} ${remap}`.trim() },
  });
  writeFileSync(provenancePath(), JSON.stringify({ version: 1, revision: source.revision, development: source.development,
    sha256: Object.fromEntries(TEAM_BINARIES.map(name => [name, hash(join(output(), 'release', name))])) }, null, 2));
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) buildTeamRuntime();
