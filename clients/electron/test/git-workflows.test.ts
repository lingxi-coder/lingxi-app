import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { GitService } from '../src/main/git';
import type { GitRequest, GitStatus } from '../src/shared/git';

const git = (cwd: string, ...args: string[]) => execFileSync('git', args, {
  cwd, encoding: 'utf8', env: { ...process.env, GIT_TERMINAL_PROMPT: '0' }, stdio: ['ignore', 'pipe', 'pipe'],
}).trim();
async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const directory = await mkdtemp(join(tmpdir(), 'lingxi-git-workflows-'));
  const root = join(directory, 'repo');
  await mkdir(root);
  git(root, 'init', '-b', 'main');
  git(root, 'config', 'user.name', 'Git Workflow Test');
  git(root, 'config', 'user.email', 'git-workflow@example.invalid');
  git(root, 'config', 'commit.gpgsign', 'false');
  await writeFile(join(root, 'file.txt'), 'base\n');
  git(root, 'add', '.'); git(root, 'commit', '-m', 'base');
  const service = new GitService();
  const scope = { projectPath: root, sessionId: 'workflow-test' };
  const request = (r: GitRequest) => service.request(scope, r);
  const status = async () => (await request({ kind: 'status' })).status as GitStatus;
  t.after(async () => { service.dispose(); await rm(directory, { recursive: true, force: true }); });
  return { directory, root, service, request, status };
}

test('remote workflow sets upstream, fetches, fast-forwards, and rejects divergent push/pull', async t => {
  const f = await fixture(t);
  const remote = join(f.directory, 'origin.git');
  git(f.directory, 'init', '--bare', remote);
  git(f.root, 'remote', 'add', 'origin', remote);
  await f.request({ kind: 'push', remote: 'origin', branch: 'main', setUpstream: true, token: (await f.status()).token });
  assert.equal((await f.status()).upstream, 'origin/main');
  const peer = join(f.directory, 'peer');
  git(f.directory, 'clone', '-b', 'main', remote, peer);
  git(peer, 'config', 'user.name', 'Peer'); git(peer, 'config', 'user.email', 'peer@example.invalid');
  git(peer, 'config', 'commit.gpgsign', 'false');
  await writeFile(join(peer, 'remote.txt'), 'remote\n');
  git(peer, 'add', '.'); git(peer, 'commit', '-m', 'remote'); git(peer, 'push');
  await f.request({ kind: 'fetch', remote: 'origin' });
  assert.equal((await f.status()).behind, 1);
  await f.request({ kind: 'pull', remote: 'origin', branch: 'main', token: (await f.status()).token });
  assert.equal(await readFile(join(f.root, 'remote.txt'), 'utf8'), 'remote\n');
  await writeFile(join(f.root, 'local.txt'), 'local\n');
  git(f.root, 'add', '.'); git(f.root, 'commit', '-m', 'local');
  await writeFile(join(peer, 'remote2.txt'), 'remote2\n');
  git(peer, 'add', '.'); git(peer, 'commit', '-m', 'remote2'); git(peer, 'push');
  await f.request({ kind: 'fetch', remote: 'origin' });
  const head = git(f.root, 'rev-parse', 'HEAD');
  await assert.rejects(f.request({ kind: 'push', remote: 'origin', branch: 'main', token: (await f.status()).token }));
  await assert.rejects(f.request({ kind: 'pull', remote: 'origin', branch: 'main', token: (await f.status()).token }));
  assert.equal(git(f.root, 'rev-parse', 'HEAD'), head);
  assert.equal(git(f.root, 'status', '--porcelain'), '');
});

async function conflictingBranches(root: string) {
  git(root, 'checkout', '-b', 'incoming');
  await writeFile(join(root, 'file.txt'), 'incoming\n');
  git(root, 'commit', '-am', 'incoming'); git(root, 'checkout', 'main');
  await writeFile(join(root, 'file.txt'), 'current\n');
  git(root, 'commit', '-am', 'current');
}

test('owned merge exposes three-way conflict, guards stale saves, resolves and commits', async t => {
  const f = await fixture(t); await conflictingBranches(f.root);
  await f.request({ kind: 'merge', branch: 'incoming', token: (await f.status()).token }).catch(() => {});
  assert.equal((await f.status()).merging, true);
  assert.equal((await f.status()).mergeOwned, true);
  const conflict = (await f.request({ kind: 'conflictRead', path: 'file.txt' })).conflict!;
  assert.equal(conflict.base, 'base\n'); assert.equal(conflict.ours, 'current\n'); assert.equal(conflict.theirs, 'incoming\n');
  await writeFile(join(f.root, 'file.txt'), 'external edit\n');
  await assert.rejects(f.request({ kind: 'conflictSave', path: 'file.txt', content: 'stale\n', token: conflict.token }));
  assert.equal(await readFile(join(f.root, 'file.txt'), 'utf8'), 'external edit\n');
  const current = (await f.request({ kind: 'conflictRead', path: 'file.txt' })).conflict!;
  await f.request({ kind: 'conflictSave', path: 'file.txt', content: 'resolved\n', token: current.token });
  assert.equal((await f.status()).files[0].conflict, true);
  await f.request({ kind: 'conflictResolve', path: 'file.txt', token: (await f.status()).token });
  await f.request({ kind: 'mergeContinue', message: 'Resolve merge', token: (await f.status()).token });
  assert.equal(git(f.root, 'show', '-s', '--format=%P').split(' ').length, 2);
  assert.equal((await f.status()).merging, false);
});

test('owned merge abort restores original head and worktree', async t => {
  const f = await fixture(t); await conflictingBranches(f.root);
  const head = git(f.root, 'rev-parse', 'HEAD');
  await f.request({ kind: 'merge', branch: 'incoming', token: (await f.status()).token }).catch(() => {});
  await f.request({ kind: 'mergeAbort', token: (await f.status()).token });
  assert.equal(git(f.root, 'rev-parse', 'HEAD'), head);
  assert.equal(await readFile(join(f.root, 'file.txt'), 'utf8'), 'current\n');
  assert.equal((await f.status()).merging, false);
});

test('stash preserves untracked option and apply retains stash until explicit drop', async t => {
  const f = await fixture(t);
  await writeFile(join(f.root, 'file.txt'), 'stashed\n');
  await writeFile(join(f.root, 'new.txt'), 'new\n');
  await f.request({ kind: 'stashCreate', message: 'tracked only', includeUntracked: false, token: (await f.status()).token });
  assert.equal(await readFile(join(f.root, 'new.txt'), 'utf8'), 'new\n');
  const stash = (await f.request({ kind: 'stashList' })).stashes![0];
  await f.request({ kind: 'stashApply', id: stash.id, token: (await f.status()).token });
  assert.equal(await readFile(join(f.root, 'file.txt'), 'utf8'), 'stashed\n');
  assert.equal((await f.request({ kind: 'stashList' })).stashes!.length, 1);
  await f.request({ kind: 'stashDrop', id: stash.id, token: (await f.status()).token });
  assert.equal((await f.request({ kind: 'stashList' })).stashes!.length, 0);
});

test('stash apply conflict retains stash and does not claim merge ownership', async t => {
  const f = await fixture(t);
  await writeFile(join(f.root, 'file.txt'), 'stash\n');
  await f.request({ kind: 'stashCreate', message: 'conflicting stash', includeUntracked: true, token: (await f.status()).token });
  const stash = (await f.request({ kind: 'stashList' })).stashes![0];
  await writeFile(join(f.root, 'file.txt'), 'committed\n'); git(f.root, 'commit', '-am', 'diverge');
  await assert.rejects(f.request({ kind: 'stashApply', id: stash.id, token: (await f.status()).token }));
  assert.equal((await f.request({ kind: 'stashList' })).stashes![0].id, stash.id);
  assert.equal((await f.status()).files[0].conflict, true);
  assert.equal((await f.status()).mergeOwned, false);
});

test('occupied worktree branch cannot be checked out', async t => {
  const f = await fixture(t);
  const other = join(f.directory, 'other');
  git(f.root, 'worktree', 'add', '-b', 'occupied', other);
  const branches = (await f.request({ kind: 'branches' })).branches!;
  assert.equal(branches.find(b => b.name === 'occupied')?.worktree, await realpath(other));
  await assert.rejects(f.request({ kind: 'checkout', branch: 'occupied', token: (await f.status()).token }));
  assert.equal((await f.status()).branch, 'main');
});

test('stale index token rejects commit without consuming draft changes', async t => {
  const f = await fixture(t);
  await writeFile(join(f.root, 'file.txt'), 'first\n'); git(f.root, 'add', '.');
  const stale = (await f.status()).token;
  await writeFile(join(f.root, 'file.txt'), 'second\n'); git(f.root, 'add', '.');
  const head = git(f.root, 'rev-parse', 'HEAD');
  await assert.rejects(f.request({ kind: 'commit', message: 'must not commit', token: stale }));
  assert.equal(git(f.root, 'rev-parse', 'HEAD'), head);
  assert.equal(git(f.root, 'show', ':file.txt'), 'second');
});

test('literal odd paths, rename, and binary files survive status and staging', async t => {
  const f = await fixture(t);
  const names = ['中文 空格.txt', 'line\nbreak.txt', ':(glob)*.txt', '-option.txt'];
  for (const name of names) await writeFile(join(f.root, name), 'text\n');
  await writeFile(join(f.root, 'binary.bin'), Buffer.from([0, 255, 0, 127]));
  const state = await f.status();
  for (const name of names) assert.ok(state.files.some(file => file.path === name), JSON.stringify(name));
  await f.request({ kind: 'stage', paths: [names[2]], token: state.token });
  assert.deepEqual(git(f.root, 'diff', '--cached', '--name-only', '-z').split('\0').filter(Boolean), [names[2]]);
  await f.request({ kind: 'stage', token: (await f.status()).token });
  const binary = (await f.request({ kind: 'diff', mode: 'staged', path: 'binary.bin' })).diff!;
  assert.equal(binary.binary, true);
  await f.request({ kind: 'commit', message: 'odd paths', token: (await f.status()).token });
  git(f.root, 'mv', '--', names[0], 'renamed 中文.txt');
  const renamed = (await f.status()).files.find(file => file.path === 'renamed 中文.txt');
  assert.equal(renamed?.oldPath, names[0]);
});

test('stale hunk cannot stage a newer edit and fresh patch stages exactly its content', async t => {
  const f = await fixture(t);
  await writeFile(join(f.root, 'file.txt'), 'first edit\n');
  const diff = (await f.request({ kind: 'diff', mode: 'working', path: 'file.txt' })).diff!;
  assert.match(diff.patch, /first edit/);
  await writeFile(join(f.root, 'file.txt'), 'second edit\n');
  await assert.rejects(f.request({ kind: 'stage', paths: ['file.txt'], patch: diff.patch, token: diff.token }));
  assert.equal(git(f.root, 'diff', '--cached'), '');
  const current = (await f.request({ kind: 'diff', mode: 'working', path: 'file.txt' })).diff!;
  await f.request({ kind: 'stage', paths: ['file.txt'], patch: current.patch, token: current.token });
  assert.equal(git(f.root, 'show', ':file.txt'), 'second edit');
});

test('removed directory files can be staged and unstaged', async t => {
  const f = await fixture(t);
  await mkdir(join(f.root, 'nested'));
  await writeFile(join(f.root, 'nested/file.txt'), 'tracked\n');
  git(f.root, 'add', '.'); git(f.root, 'commit', '-m', 'nested');
  await rm(join(f.root, 'nested'), { recursive: true });
  await f.request({ kind: 'stage', paths: ['nested/file.txt'], token: (await f.status()).token });
  assert.equal((await f.status()).files.find(file => file.path === 'nested/file.txt')?.index, 'D');
  await f.request({ kind: 'unstage', paths: ['nested/file.txt'], token: (await f.status()).token });
  assert.equal((await f.status()).files.find(file => file.path === 'nested/file.txt')?.working, 'D');
});

test('commit hook rejection keeps staged content and allows retry after fixing hook', async t => {
  const f = await fixture(t);
  await writeFile(join(f.root, 'file.txt'), 'pending commit\n'); git(f.root, 'add', '.');
  const hooks = join(f.directory, 'hooks'); await mkdir(hooks);
  git(f.root, 'config', 'core.hooksPath', hooks);
  await writeFile(join(hooks, 'pre-commit'), '#!/bin/sh\necho deliberate-hook-failure >&2\nexit 1\n', { mode: 0o755 });
  const head = git(f.root, 'rev-parse', 'HEAD');
  await assert.rejects(f.request({ kind: 'commit', message: 'Retained message', token: (await f.status()).token }), /deliberate-hook-failure/);
  assert.equal(git(f.root, 'rev-parse', 'HEAD'), head);
  assert.equal(git(f.root, 'show', ':file.txt'), 'pending commit');
  await rm(join(hooks, 'pre-commit'));
  await f.request({ kind: 'commit', message: 'Retained message', token: (await f.status()).token });
  assert.equal(git(f.root, 'log', '-1', '--format=%s'), 'Retained message');
});

test('occupied index lock rejects staging without overwriting the existing index', async t => {
  const f = await fixture(t);
  await writeFile(join(f.root, 'file.txt'), 'locked edit\n');
  const index = await readFile(join(f.root, '.git/index'));
  const token = (await f.status()).token;
  await writeFile(join(f.root, '.git/index.lock'), 'another Git process');
  await assert.rejects(f.request({ kind: 'stage', paths: ['file.txt'], token }), /index.lock|another git process/i);
  assert.deepEqual(await readFile(join(f.root, '.git/index')), index);
  assert.equal(await readFile(join(f.root, '.git/index.lock'), 'utf8'), 'another Git process');
  await rm(join(f.root, '.git/index.lock'));
  await f.request({ kind: 'stage', paths: ['file.txt'], token: (await f.status()).token });
  assert.equal(git(f.root, 'show', ':file.txt'), 'locked edit');
});

test('failed signing preserves HEAD and index and reports a terminal recovery path', async t => {
  const f = await fixture(t);
  const signer = join(f.directory, 'reject-signing');
  await writeFile(signer, '#!/bin/sh\necho deliberate-signing-failure >&2\nexit 1\n', { mode: 0o755 });
  git(f.root, 'config', 'commit.gpgsign', 'true');
  git(f.root, 'config', 'gpg.format', 'openpgp');
  git(f.root, 'config', 'gpg.program', signer);
  await writeFile(join(f.root, 'file.txt'), 'unsigned edit\n'); git(f.root, 'add', '.');
  const head = git(f.root, 'rev-parse', 'HEAD');
  const stagedTree = git(f.root, 'write-tree');
  await assert.rejects(f.request({ kind: 'commit', message: 'Signing must fail', token: (await f.status()).token }), /terminal/i);
  assert.equal(git(f.root, 'rev-parse', 'HEAD'), head);
  assert.equal(git(f.root, 'write-tree'), stagedTree);
  assert.equal(git(f.root, 'show', ':file.txt'), 'unsigned edit');
});

test('empty initialized repository exposes unborn branch and accepts first staged commit', async t => {
  const f = await fixture(t);
  const empty = join(f.directory, 'empty'); await mkdir(empty);
  git(empty, 'init', '-b', 'first');
  git(empty, 'config', 'user.name', 'Initial Test'); git(empty, 'config', 'user.email', 'initial@example.invalid');
  git(empty, 'config', 'commit.gpgsign', 'false');
  const scope = { projectPath: empty, sessionId: 'initial' };
  const before = (await f.service.request(scope, { kind: 'status' })).status!;
  assert.equal(before.repository, true); assert.equal(before.branch, 'first'); assert.equal(before.head, '');
  assert.deepEqual((await f.service.request(scope, { kind: 'history' })).commits, []);
  await writeFile(join(empty, 'initial.txt'), 'initial\n');
  const status = async () => (await f.service.request(scope, { kind: 'status' })).status!;
  await f.service.request(scope, { kind: 'stage', paths: ['initial.txt'], token: (await status()).token });
  await f.service.request(scope, { kind: 'commit', message: 'First commit', token: (await status()).token });
  assert.match((await status()).head, /^[a-f0-9]{40,64}$/);
});

test('detached HEAD remains explicit and history plus commit diff are readable', async t => {
  const f = await fixture(t);
  git(f.root, 'checkout', '--detach', 'HEAD');
  const status = await f.status();
  assert.match(status.branch, /detached/i);
  assert.equal(status.head, git(f.root, 'rev-parse', 'HEAD'));
  assert.equal((await f.request({ kind: 'history' })).commits![0].id, status.head);
  const diff = (await f.request({ kind: 'diff', mode: 'commit', target: status.head })).diff!;
  assert.match(diff.patch, /base/);
});

test('truncated large diff is bounded and cannot stage a displayed hunk', async t => {
  const f = await fixture(t);
  const original = 'base\n' + Array.from({ length: 120_000 }, (_, i) => `original ${i} xxxxxxxxxxxxxxxxxxxx\n`).join('');
  await writeFile(join(f.root, 'file.txt'), original); git(f.root, 'commit', '-am', 'large base');
  const edited = original.replaceAll('original', 'modified');
  await writeFile(join(f.root, 'file.txt'), edited);
  const diff = (await f.request({ kind: 'diff', mode: 'working', path: 'file.txt' })).diff!;
  assert.equal(diff.truncated, true);
  assert.ok(Buffer.byteLength(diff.patch) <= 2 * 1024 * 1024);
  const header = diff.patch.slice(0, diff.patch.indexOf('@@'));
  await assert.rejects(f.request({ kind: 'stage', paths: ['file.txt'], patch: `${header}@@ -1 +1 @@\n-base\n+replacement\n`, token: diff.token }), /truncated/i);
  assert.equal(git(f.root, 'diff', '--cached'), '');
});

test('conflict read and save reject a symlink to an external file', async t => {
  const f = await fixture(t); await conflictingBranches(f.root);
  await f.request({ kind: 'merge', branch: 'incoming', token: (await f.status()).token }).catch(() => {});
  const external = join(f.directory, 'private.txt'); await writeFile(external, 'outside repository\n');
  await rm(join(f.root, 'file.txt')); await symlink(external, join(f.root, 'file.txt'));
  await assert.rejects(f.request({ kind: 'conflictRead', path: 'file.txt' }), /symbolic|symlink/i);
  await assert.rejects(f.request({ kind: 'conflictSave', path: 'file.txt', content: 'overwrite', token: (await f.status()).token }), /symbolic|symlink/i);
  assert.equal(await readFile(external, 'utf8'), 'outside repository\n');
});

test('missing Git reports unavailable status in an isolated subprocess', async t => {
  const f = await fixture(t);
  const loader = new URL('../../shared/node_modules/tsx/dist/loader.mjs', import.meta.url);
  const serviceUrl = new URL('../src/main/git.ts', import.meta.url).href;
  const script = `import { GitService } from ${JSON.stringify(serviceUrl)}; process.env.PATH = ''; const service = new GitService(); try { const result = await service.request(${JSON.stringify({ projectPath: f.root, sessionId: 'missing-git' })}, {kind:'status'}); process.stdout.write(JSON.stringify(result.status)); } finally { service.dispose(); }`;
  const output = execFileSync(process.execPath, ['--import', loader.href, '--input-type=module', '--eval', script], { encoding: 'utf8' });
  const result = JSON.parse(output);
  assert.equal(result.available, false); assert.equal(result.repository, false);
});

test('discarding an untracked nested repository removes it instead of failing with EISDIR', async t => {
  const f = await fixture(t);
  // `git status -uall` does NOT descend into a nested repository: it reports the
  // whole thing as one entry with a trailing slash, which a non-recursive `rm`
  // rejects. The nested repo is the realistic shape (a stray clone, a vendored
  // checkout), and it is exactly the one the file-by-file delete cannot handle.
  await mkdir(join(f.root, 'vendor', 'thing'), { recursive: true });
  git(join(f.root, 'vendor', 'thing'), 'init');
  await writeFile(join(f.root, 'vendor', 'thing', 'inner.txt'), 'inner\n');
  const entry = (await f.status()).files.find(file => file.path.startsWith('vendor/'));
  assert.ok(entry?.untracked, `the nested repo must appear as one untracked entry: ${entry?.path}`);
  await f.request({ kind: 'discard', paths: [entry.path], untracked: true, token: (await f.status()).token });
  assert.deepEqual((await f.status()).files.filter(file => file.path.startsWith('vendor/')), []);
  await assert.rejects(readFile(join(f.root, 'vendor', 'thing', 'inner.txt'), 'utf8'), /ENOENT/);
});

test('a Git child never inherits host credentials, and an unresolvable Git fails as ENOENT', async t => {
  const f = await fixture(t);
  const loader = new URL('../../shared/node_modules/tsx/dist/loader.mjs', import.meta.url);
  const serviceUrl = new URL('../src/main/git.ts', import.meta.url).href;
  // Git runs repository-controlled hooks and credential helpers, so what its
  // child can read is a security boundary — asserted through a real spawn (a
  // hook the repository controls records the environment it was handed) rather
  // than by reading the allowlist back, which would only restate the source.
  const hooks = join(f.root, '.git', 'hooks');
  await mkdir(hooks, { recursive: true });
  const record = join(f.directory, 'hook-env.txt');
  await writeFile(join(hooks, 'post-checkout'), `#!/bin/sh\nenv > ${JSON.stringify(record)}\n`, { mode: 0o755 });
  process.env.ANTHROPIC_API_KEY = 'leaked-key';
  process.env.NODE_OPTIONS = '--require /tmp/injected.js';
  try {
    git(f.root, 'branch', 'other');
    await f.request({ kind: 'checkout', branch: 'other', token: (await f.status()).token });
    const seen = await readFile(record, 'utf8');
    assert.ok(!seen.includes('leaked-key'), 'a repository hook must not receive the host API key');
    assert.ok(!seen.includes('/tmp/injected.js'), 'a repository hook must not receive NODE_OPTIONS');
    assert.match(seen, /^PATH=/m, '…while the variables Git legitimately needs are still present');
  } finally {
    delete process.env.ANTHROPIC_API_KEY;
    delete process.env.NODE_OPTIONS;
  }
  // And when Git cannot be resolved at all, the failure is ENOENT — which is what
  // makes `status` report `available: false` instead of throwing at the panel.
  const script = `import { GitService } from ${JSON.stringify(serviceUrl)}; process.env.PATH = ''; const service = new GitService(); try { const result = await service.request(${JSON.stringify({ projectPath: f.root, sessionId: 'missing-git' })}, {kind:'status'}); process.stdout.write(JSON.stringify(result.status)); } finally { service.dispose(); }`;
  const output = execFileSync(process.execPath, ['--import', loader.href, '--input-type=module', '--eval', script], { encoding: 'utf8' });
  assert.equal(JSON.parse(output).available, false);
});
