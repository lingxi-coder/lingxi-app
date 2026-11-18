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

test('sync checks workflows at the supplied workspace pin even when its Harness checkout has advanced', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'lingxi-workflow-pin-'));
  const product = path.join(root, 'lingxi-app');
  const harness = path.join(root, 'harness-runtime');
  const workflowPath = path.join(harness, 'crates/skill/src/builtin/lingxi-api/workflows.json');
  const web = fileURLToPath(new URL('..', import.meta.url));
  try {
    mkdirSync(product);
    mkdirSync(path.dirname(workflowPath), { recursive: true });
    run('git', ['init', '-q'], harness);
    run('git', ['config', 'user.name', 'Documentation test'], harness);
    run('git', ['config', 'user.email', 'docs@example.invalid'], harness);
    writeFileSync(workflowPath, '[]\n');
    run('git', ['add', '.'], harness);
    run('git', ['commit', '-qm', 'pinned workflows'], harness);
    const pin = run('git', ['rev-parse', 'HEAD'], harness).trim();
    writeFileSync(path.join(product, 'Cargo.lock'), `[[package]]\nname = "skill"\nsource = "git+https://github.com/lingxi-coder/harness-runtime.git?rev=${pin}#${pin}"\n`);
    writeFileSync(workflowPath, readFileSync(path.join(web, 'src/data/docs/product-workflows.json')));
    run('git', ['commit', '-qam', 'match current handbook at HEAD'], harness);
    // Only the target pin has the old index. Using either HEAD or the script's own
    // product lock would miss the stale snapshot or fail to find that revision.
    const check = spawnSync(process.execPath, [path.join(web, 'scripts/sync-docs.mjs'), '--check', '--workspace', '..'], { cwd: product, encoding: 'utf8' });
    assert.equal(check.status, 1);
    assert.match(check.stderr, /^Error: Product workflows are stale\./mu);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

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
      run('git', ['update-ref', 'refs/remotes/origin/main', 'HEAD'], directory);
    }
    const pages = JSON.stringify([{ id: 'example', sources: repos.map((repo) => ({ repo, path: 'config.toml' })) }]);
    const generate = () => { run('python3', [script, '--workspace', root, '--output', output], root, pages); return JSON.parse(readFileSync(output, 'utf8')); };
    const committed = generate();
    assert.ok(committed.pages.example.every((source: { sourceState: string; url?: string }) => source.sourceState === 'committed' && source.url));
    const product = path.join(root, 'lingxi-app');
    const sourceRevision = run('git', ['rev-parse', 'HEAD'], product).trim();
    writeFileSync(path.join(product, 'README.md'), 'Documentation-only commit.\n');
    run('git', ['add', 'README.md'], product);
    run('git', ['commit', '-qm', 'documentation only'], product);
    assert.notEqual(run('git', ['rev-parse', 'HEAD'], product).trim(), sourceRevision);
    const afterDocOnlyCommit = generate();
    assert.equal(afterDocOnlyCommit.repositories.find((repo: { repo: string }) => repo.repo === 'lingxi-app').revision, sourceRevision);
    assert.equal(afterDocOnlyCommit.pages.example[0].revision, sourceRevision);
    assert.deepEqual(afterDocOnlyCommit, committed);
    writeFileSync(path.join(root, 'llm-client', 'config.toml'), 'enabled = false\n');
    run('git', ['add', 'config.toml'], path.join(root, 'llm-client'));
    const changed = generate();
    assert.notEqual(changed.repositories.find((repo: { repo: string }) => repo.repo === 'llm-client').treeFingerprint, committed.repositories.find((repo: { repo: string }) => repo.repo === 'llm-client').treeFingerprint);
    assert.equal(changed.repositories.find((repo: { repo: string }) => repo.repo === 'llm-client').dirty, true);
    assert.equal(changed.pages.example[2].url, undefined);
    assert.equal(changed.repositories[2].productPinnedRevision, '1'.repeat(40));
    const first = readFileSync(output, 'utf8'); generate(); assert.equal(readFileSync(output, 'utf8'), first);
    writeFileSync(path.join(root, 'llm-client', 'not-referenced.txt'), 'unrelated change'); generate(); assert.equal(readFileSync(output, 'utf8'), first);
    writeFileSync(path.join(root, 'llm-client', 'config.toml'), 'enabled = true\n');
    assert.deepEqual(generate(), committed);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('handbook distinguishes unpublished commits from dirty files and recognizes published ancestors', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'lingxi-handbook-publication-'));
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
      if (repo === 'llm-client') writeFileSync(path.join(directory, 'stable.toml'), 'public = true\n');
      if (repo === 'lingxi-app') writeFileSync(path.join(directory, 'Cargo.toml'), '[workspace.dependencies]\n');
      run('git', ['add', '.'], directory);
      run('git', ['commit', '-qm', 'published baseline'], directory);
      run('git', ['update-ref', 'refs/remotes/origin/main', 'HEAD'], directory);
    }
    const pages = JSON.stringify([{ id: 'example', sources: [
      ...repos.map((repo) => ({ repo, path: 'config.toml' })), { repo: 'llm-client', path: 'stable.toml' },
    ] }]);
    const generate = () => {
      run('python3', [script, '--workspace', root, '--output', output], root, pages);
      return JSON.parse(readFileSync(output, 'utf8'));
    };
    const local = path.join(root, 'llm-client');
    writeFileSync(path.join(local, 'config.toml'), 'enabled = false\n');
    run('git', ['add', 'config.toml'], local);
    run('git', ['commit', '-qm', 'unpublished source'], local);
    const unpublishedRevision = run('git', ['rev-parse', 'HEAD'], local).trim();
    let baseline = generate();
    let repository = baseline.repositories.find((repo: { repo: string }) => repo.repo === 'llm-client');
    assert.equal(repository.revision, unpublishedRevision);
    assert.equal(repository.published, false);
    assert.equal(repository.dirty, false);
    assert.equal(baseline.pages.example[2].sourceState, 'unpublished');
    assert.equal(baseline.pages.example[2].url, undefined);
    const publicRevision = run('git', ['rev-parse', 'origin/main'], local).trim();
    assert.equal(baseline.pages.example[4].sourceState, 'committed');
    assert.ok(baseline.pages.example[4].url.endsWith(`/blob/${publicRevision}/stable.toml`));
    assert.ok(baseline.pages.example.filter((source: { repo: string }) => source.repo !== 'llm-client').every((source: { url?: string }) => source.url));

    writeFileSync(path.join(local, 'config.toml'), 'enabled = "working"\n');
    baseline = generate();
    assert.equal(baseline.pages.example[2].sourceState, 'working-tree');
    assert.equal(baseline.pages.example[2].url, undefined);
    assert.equal(baseline.repositories.find((repo: { repo: string }) => repo.repo === 'llm-client').dirty, true);
    run('git', ['restore', 'config.toml'], local);
    run('git', ['update-ref', 'refs/remotes/origin/main', 'HEAD'], local);
    baseline = generate();
    repository = baseline.repositories.find((repo: { repo: string }) => repo.repo === 'llm-client');
    assert.equal(repository.published, true);
    assert.equal(repository.dirty, false);
    assert.equal(baseline.pages.example[2].sourceState, 'committed');
    assert.equal(baseline.pages.example[2].url,
      `https://github.com/lingxi-coder/llm-client/blob/${unpublishedRevision}/config.toml`);

    // Product baseline revisions follow referenced source, so a published
    // descendant can establish availability of the older selected revision.
    const product = path.join(root, 'lingxi-app');
    const productRevision = run('git', ['rev-parse', 'HEAD'], product).trim();
    writeFileSync(path.join(product, 'README.md'), 'published docs\n');
    run('git', ['add', 'README.md'], product);
    run('git', ['commit', '-qm', 'published descendant'], product);
    run('git', ['update-ref', 'refs/remotes/origin/main', 'HEAD'], product);
    writeFileSync(path.join(product, 'README.md'), 'local docs\n');
    run('git', ['add', 'README.md'], product);
    run('git', ['commit', '-qm', 'unpublished descendant'], product);
    baseline = generate();
    assert.equal(baseline.pages.example[0].revision, productRevision);
    assert.equal(baseline.pages.example[0].sourceState, 'committed');
    assert.ok(baseline.pages.example[0].url);

    run('git', ['update-ref', '-d', 'refs/remotes/origin/main'], path.join(root, 'mobile-linux-runtime'));
    baseline = generate();
    assert.equal(baseline.pages.example[3].sourceState, 'unpublished');
    assert.equal(baseline.pages.example[3].url, undefined);
    assert.equal(baseline.repositories.find((repo: { repo: string }) => repo.repo === 'mobile-linux-runtime').dirty, false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
