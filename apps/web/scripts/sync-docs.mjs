import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const webRoot = fileURLToPath(new URL('..', import.meta.url));
const args = process.argv.slice(2);
const check = args.includes('--check');
const contentOnly = args.includes('--content-only');
const position = args.indexOf('--workspace');
const workspace = position >= 0 ? args[position + 1] : path.resolve(webRoot, '../../..');
if (position >= 0 && !workspace) throw new Error('--workspace requires a path');
let failed = false;
function run(command, argv, input) {
  const result = spawnSync(command, argv, { cwd: webRoot, encoding: 'utf8', input, stdio: input === undefined ? 'inherit' : ['pipe', 'inherit', 'inherit'] });
  if (result.error) throw result.error;
  if (result.status !== 0) failed = true;
}
if (!contentOnly) {
  const refs = spawnSync(process.execPath, ['--import', 'tsx', 'scripts/generate-handbook.ts', '--sources'], { cwd: webRoot, encoding: 'utf8' });
  if (refs.status !== 0) { process.stderr.write(refs.stderr); process.exit(refs.status || 1); }
  run('python3', ['scripts/generate-doc-baseline.py', '--workspace', workspace, ...(check ? ['--check'] : [])], refs.stdout);
  run('python3', ['scripts/generate-api-index.py', '--workspace', workspace, '--source-mode', 'working-tree', ...(check ? ['--check'] : [])]);
}
if (!failed) run('python3', ['scripts/check-doc-examples.py', check ? '--check-fixtures' : '--sync-fixtures']);
if (!failed) run(process.execPath, ['--import', 'tsx', 'scripts/generate-handbook.ts', ...(check ? ['--check'] : [])]);
process.exit(failed ? 1 : 0);
