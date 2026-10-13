import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
const script = fileURLToPath(new URL('../scripts/generate-doc-baseline.py', import.meta.url));
function run(command: string, args: string[], cwd: string, input?: string) {
  const result = spawnSync(command, args, { cwd, input, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr + result.stdout);
  return result.stdout;
}

test('handbook fingerprints track referenced configuration independently of API selection', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'lingxi-handbook-baseline-'));
  const repos = ['lingxi-app', 'harness-runtime', 'llm-client', 'mobile-linux-runtime'];
  const output = path.join(root, 'baseline.json');
  try {
    for (const repo of repos) {
      const directory = path.join(root, repo); mkdirSync(directory);
      run('git', ['init', '-q'], directory);
      run('git', ['config', 'user.name', 'Documentation test'], directory);
      run('git', ['config', 'user.email', 'docs@example.invalid'], directory);
      run('git', ['remote', 'add', 'origin', `https://github.com/lingxi-coder/${repo}.git`], directory);
      writeFileSync(path.join(directory, 'config.toml'), 'enabled = true\n');
      if (repo === 'lingxi-app') writeFileSync(path.join(directory, 'Cargo.toml'), '[workspace.dependencies]\n' + repos.slice(1).map((name) => `${name} = { git = "https://github.com/lingxi-coder/${name}.git", rev = "${'1'.repeat(40)}" }`).join('\n') + '\n');
      run('git', ['add', '.'], directory); run('git', ['commit', '-qm', 'fixture'], directory);
    }
    const pages = JSON.stringify([{ id: 'example', sources: repos.map((repo) => ({ repo, path: 'config.toml' })) }]);
    const generate = () => { run('python3', [script, '--workspace', root, '--output', output], root, pages); return JSON.parse(readFileSync(output, 'utf8')); };
    const committed = generate();
    assert.ok(committed.pages.example.every((source: { sourceState: string; url?: string }) => source.sourceState === 'committed' && source.url));
    writeFileSync(path.join(root, 'llm-client', 'config.toml'), 'enabled = false\n');
    run('git', ['add', 'config.toml'], path.join(root, 'llm-client'));
    const changed = generate();
    assert.notEqual(changed.repositories.find((repo: { repo: string }) => repo.repo === 'llm-client').treeFingerprint, committed.repositories.find((repo: { repo: string }) => repo.repo === 'llm-client').treeFingerprint);
    assert.equal(changed.pages.example[2].url, undefined);
    assert.equal(changed.repositories[2].productPinnedRevision, '1'.repeat(40));
    const first = readFileSync(output, 'utf8'); generate(); assert.equal(readFileSync(output, 'utf8'), first);
    writeFileSync(path.join(root, 'llm-client', 'not-referenced.txt'), 'unrelated change'); generate(); assert.equal(readFileSync(output, 'utf8'), first);
    writeFileSync(path.join(root, 'llm-client', 'config.toml'), 'enabled = true\n');
    assert.deepEqual(generate(), committed);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
