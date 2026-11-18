import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const webRoot = fileURLToPath(new URL('..', import.meta.url));
const args = process.argv.slice(2);
const check = args.includes('--check');
const contentOnly = args.includes('--content-only');
const position = args.indexOf('--workspace');
if (position >= 0 && !args[position + 1]) throw new Error('--workspace requires a path');
const workspace = position >= 0 ? path.resolve(args[position + 1]) : path.resolve(webRoot, '../../..');
let failed = false;
function run(command, argv, input) {
  const result = spawnSync(command, argv, { cwd: webRoot, encoding: 'utf8', input, stdio: input === undefined ? 'inherit' : ['pipe', 'inherit', 'inherit'] });
  if (result.error) throw result.error;
  if (result.status !== 0) failed = true;
}
if (!contentOnly) {
  // The shipped skill and the handbook search must use the same workflow IDs
  // even when the neighbouring Harness checkout is on a newer branch.
  const lock = readFileSync(path.join(workspace, 'lingxi-app', 'Cargo.lock'), 'utf8');
  const revisions = new Set(Array.from(lock.matchAll(/source = "git\+https:\/\/github\.com\/lingxi-coder\/harness-runtime\.git[^"#]*#([a-f0-9]{40})"/g), match => match[1]));
  if (revisions.size !== 1) throw new Error('Expected one pinned Harness revision in Cargo.lock');
  const [revision] = revisions;
  const canonical = spawnSync('git', ['-C', path.join(workspace, 'harness-runtime'), 'show',
    revision + ':crates/skill/src/builtin/lingxi-api/workflows.json'], { encoding: 'utf8' });
  if (canonical.status !== 0) throw new Error('Cannot read the product workflow index at Harness revision ' + revision);
  const snapshot = path.join(webRoot, 'src/data/docs/product-workflows.json');
  if (check) {
    if (readFileSync(snapshot, 'utf8') !== canonical.stdout) throw new Error('Product workflows are stale. Run npm run docs:sync');
  } else {
    writeFileSync(snapshot, canonical.stdout);
  }
  const refs = spawnSync(process.execPath, ['--import', 'tsx', 'scripts/generate-handbook.ts', '--sources'], { cwd: webRoot, encoding: 'utf8' });
  if (refs.status !== 0) { process.stderr.write(refs.stderr); process.exit(refs.status || 1); }
  run('python3', ['scripts/generate-doc-baseline.py', '--workspace', workspace, ...(check ? ['--check'] : [])], refs.stdout);
  run('python3', ['scripts/generate-api-index.py', '--workspace', workspace, '--source-mode', 'working-tree', ...(check ? ['--check'] : [])]);
}
if (!failed) run('python3', ['scripts/check-doc-examples.py', check ? '--check-fixtures' : '--sync-fixtures']);
if (!failed) run(process.execPath, ['--import', 'tsx', 'scripts/generate-handbook.ts', ...(check ? ['--check'] : [])]);
process.exit(failed ? 1 : 0);
