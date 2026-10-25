import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

interface ApiEntry {
  sdk: string;
  name: string;
  kind: string;
  signature: string;
  summary: string;
  file: string;
  line: number;
  sourceState: 'committed' | 'unpublished' | 'working-tree';
  url?: string;
}

interface ApiRepository {
  sdk: string;
  revision: string;
  sourceUrl: string;
  published?: boolean;
  dirty: boolean;
  treeFingerprint: string;
  productPinnedRevision?: string;
}

interface ApiCatalog {
  generatedAt: string;
  sourceMode: 'committed' | 'working-tree';
  repositories: ApiRepository[];
  entries: ApiEntry[];
}

const script = fileURLToPath(new URL('../scripts/generate-api-index.py', import.meta.url));
const catalog = JSON.parse(readFileSync(new URL('../public/api-index.json', import.meta.url), 'utf8')) as ApiCatalog;

test('SDK catalog has usable declarations and links only byte-identical committed sources', () => {
  assert.match(catalog.generatedAt, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/);
  assert.ok(['committed', 'working-tree'].includes(catalog.sourceMode));
  assert.deepEqual(catalog.repositories.map((repo) => repo.sdk).sort(), ['bridge', 'harness', 'llm', 'mobile']);
  const repositories = new Map(catalog.repositories.map((repo) => [repo.sdk, repo]));
  const identities = new Set<string>();
  for (const repo of catalog.repositories) {
    assert.match(repo.revision, /^[a-f0-9]{40}$/);
    assert.match(repo.sourceUrl, /^https:\/\/github\.com\/lingxi-coder\/[\w-]+$/);
    assert.equal(typeof repo.dirty, 'boolean');
    assert.match(repo.treeFingerprint, /^sha256:[a-f0-9]{64}$/);
    if (catalog.sourceMode === 'committed') assert.equal(repo.dirty, false);
    assert.ok(catalog.entries.filter((entry) => entry.sdk === repo.sdk).length > 10, repo.sdk);
  }
  assert.ok(catalog.entries.some((entry) => entry.sdk === 'harness' && entry.file.startsWith('crates/skill/src/')),
    'Harness API entries must follow the current skill crate path');
  assert.ok(!catalog.entries.some((entry) => entry.sdk === 'harness' && entry.file.startsWith('crates/skill-api/src/')),
    'the renamed skill-api source path is not part of the current API index');
  for (const entry of catalog.entries) {
    const repo = repositories.get(entry.sdk);
    assert.ok(repo, `unknown SDK: ${entry.sdk}`);
    assert.ok(entry.name && entry.kind && entry.signature, JSON.stringify(entry));
    assert.equal(typeof entry.summary, 'string');
    assert.ok(Number.isInteger(entry.line) && entry.line > 0);
    if (entry.sourceState === 'committed') {
      assert.notEqual(repo.published, false);
      assert.equal(entry.url, `${repo.sourceUrl}/blob/${repo.revision}/${entry.file}#L${entry.line}`);
    } else if (entry.sourceState === 'unpublished') {
      assert.equal(repo.published, false);
      assert.equal(entry.url, undefined);
    } else {
      assert.equal(entry.sourceState, 'working-tree');
      assert.equal(catalog.sourceMode, 'working-tree');
      assert.equal(entry.url, undefined);
    }
    assert.doesNotMatch(entry.file, /(^|\/)(tests?|fixtures?|vendor|snapshots|examples)(\/|\.)|(_tests?|test_support)\.(rs|ts)$/);
    assert.doesNotMatch(entry.signature, /^pub\s*\([^)]*\)|^(private|protected|internal)\b/);
    const identity = `${entry.sdk}:${entry.file}:${entry.line}:${entry.name}`;
    assert.equal(identities.has(identity), false, identity);
    identities.add(identity);
  }
});

test('bridge catalog revision remains stable across its own index commit and follows API source changes', (t) => {
  const { workspace } = fixture(t);
  const bridge = join(workspace, 'lingxi-app');
  const output = join(bridge, 'apps/web/public/api-index.json');
  mkdirSync(dirname(output), { recursive: true });
  const generate = (check = false) => {
    const args = [script, '--workspace', workspace, '--output', output];
    if (check) args.push('--check');
    return spawnSync('python3', args, { encoding: 'utf8' });
  };

  const generated = generate();
  assert.equal(generated.status, 0, generated.stderr);
  const sourceRevision = git(bridge, 'log', '-1', '--format=%H', 'HEAD', '--', 'packages/bridge-client/src');
  const initialHead = git(bridge, 'rev-parse', 'HEAD');
  const initialIndex = JSON.parse(readFileSync(output, 'utf8')) as ApiCatalog;
  assert.equal(initialIndex.repositories.find((repo) => repo.sdk === 'bridge')?.revision, sourceRevision);
  assert.equal(sourceRevision, initialHead);
  assert.equal(generate(true).status, 0);

  git(bridge, 'add', 'apps/web/public/api-index.json');
  git(bridge, 'commit', '-qm', 'commit generated API index');
  assert.notEqual(git(bridge, 'rev-parse', 'HEAD'), initialHead);
  assert.equal(generate(true).status, 0, 'the catalog commit must not change its indexed bridge source revision');

  writeFileSync(join(bridge, 'packages/bridge-client/src/protocol.ts'), 'export interface BridgeBase { value: number }\n');
  git(bridge, 'add', 'packages/bridge-client/src/protocol.ts');
  git(bridge, 'commit', '-qm', 'change bridge API source');
  const changedSourceRevision = git(bridge, 'log', '-1', '--format=%H', 'HEAD', '--', 'packages/bridge-client/src');
  assert.notEqual(changedSourceRevision, sourceRevision);
  const stale = generate(true);
  assert.notEqual(stale.status, 0);
  assert.match(stale.stderr, /API catalog is stale/);

  assert.equal(generate().status, 0);
  assert.equal(generate(true).status, 0);
  const updatedIndex = JSON.parse(readFileSync(output, 'utf8')) as ApiCatalog;
  assert.equal(updatedIndex.repositories.find((repo) => repo.sdk === 'bridge')?.revision, changedSourceRevision);
});

test('catalog includes lifecycle traits, cross-file methods, bridge methods, and native facades', () => {
  for (const [sdk, name] of [
    ['harness', 'HarnessBuilder::new'],
    ['harness', 'SessionService::run'],
    ['harness', 'Harness::shutdown'],
    ['llm', 'LlmClientBuilder::new'],
    ['llm', 'ClientSnapshot::chat'],
    ['bridge', 'BridgeClient.connect'],
    ['bridge', 'BridgeClient.discoveredLockfile'],
    ['mobile', 'MobileLinuxRuntime.boot'],
    ['mobile', 'RootfsInstaller.stage'],
  ]) {
    assert.ok(catalog.entries.some((entry) => entry.sdk === sdk && entry.name === name), `${sdk} ${name}`);
  }
  const lifecycle = catalog.entries.find((entry) => entry.sdk === 'harness' && entry.name === 'SessionService::run');
  assert.ok(lifecycle);
  assert.match(lifecycle.signature, /input: RunInput,[\s\S]*cancel: CancellationToken,[\s\S]*\) -> Result<TurnOutcome, HandleError>;/);
  assert.match(lifecycle.summary, /Execute a turn/);
  assert.ok(catalog.entries.some((entry) => entry.sdk === 'mobile' && entry.file.endsWith('.swift')));
  assert.ok(catalog.entries.some((entry) => entry.sdk === 'mobile' && entry.file.endsWith('.kt')));
});

function extract(source: string, language: string): Array<[string, string, string, string, number]> {
  const result = spawnSync('python3', ['-c', [
    'import json, runpy, sys',
    'module = runpy.run_path(sys.argv[1])',
    'source = sys.stdin.read()',
    "result = module['rust_declarations'](source) if sys.argv[2] == 'rust' else module['native_declarations'](source, sys.argv[2])",
    'print(json.dumps(result))',
  ].join('\n'), script, language], { input: source, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(result.stdout) as Array<[string, string, string, string, number]>;
}

test('source extractor balances multiline signatures and excludes test/private implementation declarations', () => {
  const declarations = extract(`
/// Public facade.
pub struct Client {}
struct Internal {}
impl Client {
    /// Receives the complete request.
    pub async fn run(
        &self,
        input: (String, Vec<(u32, u32)>),
    ) -> Result<(String, usize), Error> {
        pub struct LocalOnly {}
        todo!()
    }
    pub const fn limit(&self) -> usize { 8 }
    pub(crate) fn hidden(&self) {}
}
impl Internal {
    pub fn implementation_only(&self) {}
}
/// Host implementation contract.
pub trait Service {
    /// Supplies the caller's request.
    async fn execute(&self, request: Request) -> Result<(), Error>;
    fn defaulted(&self) -> bool { true }
}
#[cfg(test)]
mod tests {
    pub struct Fixture {}
    impl Client { pub fn test_helper(&self) {} }
}
`, 'rust');
  assert.deepEqual(declarations.map(([name]) => name), ['Client', 'Client::run', 'Client::limit', 'Service', 'Service::execute', 'Service::defaulted']);
  const run = declarations.find(([name]) => name === 'Client::run');
  assert.ok(run);
  assert.match(run[2], /Vec<\(u32, u32\)>\),[\s\S]*\) -> Result<\(String, usize\), Error>$/);
  assert.equal(run[3], 'Receives the complete request.');
  assert.equal(declarations.find(([name]) => name === 'Client::limit')?.[1], 'method');
});

const fixtureRepositories = [
  ['harness', 'harness-runtime', 'crates/runtime/src/api.rs', 'pub struct HarnessBase {}\n'],
  ['llm', 'llm-client', 'src/lib.rs', 'pub struct LlmBase {}\n'],
  ['mobile', 'mobile-linux-runtime', 'crates/mobile-linux-api/src/lib.rs', 'pub struct MobileBase {}\n'],
  ['bridge', 'lingxi-app', 'packages/bridge-client/src/protocol.ts', 'export interface BridgeBase {}\n'],
] as const;

function git(repo: string, ...args: string[]): string {
  const result = spawnSync('git', ['-C', repo, '-c', 'commit.gpgsign=false', ...args], {
    encoding: 'utf8',
    env: { ...process.env, GIT_AUTHOR_NAME: 'API fixture', GIT_AUTHOR_EMAIL: 'fixture@example.invalid', GIT_COMMITTER_NAME: 'API fixture', GIT_COMMITTER_EMAIL: 'fixture@example.invalid', GIT_AUTHOR_DATE: '2026-01-01T00:00:00Z', GIT_COMMITTER_DATE: '2026-01-01T00:00:00Z' },
  });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

function write(path: string, content: string): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}

function fixture(t: { after: (cleanup: () => void) => void }) {
  const workspace = mkdtempSync(join(tmpdir(), 'lingxi-api-sources-'));
  t.after(() => rmSync(workspace, { recursive: true, force: true }));
  for (const [, directory, file, content] of fixtureRepositories) {
    const repo = join(workspace, directory);
    mkdirSync(repo);
    git(repo, 'init', '-q');
    git(repo, 'remote', 'add', 'origin', `git@github.com-lingxi-coder:lingxi-coder/${directory}.git`);
    write(join(repo, file), content);
    write(join(repo, '.gitignore'), 'src/ignored.rs\n');
    if (directory === 'lingxi-app') {
      write(join(repo, 'Cargo.toml'), `[workspace.dependencies]\nlingxi-llm-client = { git = "https://github.com/lingxi-coder/llm-client.git", rev = "${'a'.repeat(40)}" }\n`);
    }
    git(repo, 'add', '.');
    git(repo, 'commit', '-qm', 'API baseline');
    git(repo, 'update-ref', 'refs/remotes/origin/main', 'HEAD');
  }
  const generate = (sourceMode: ApiCatalog['sourceMode']) => {
    const output = join(workspace, `api-${sourceMode}.json`);
    const result = spawnSync('python3', [script, '--workspace', workspace, '--output', output, '--source-mode', sourceMode], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    const bytes = readFileSync(output, 'utf8');
    return { catalog: JSON.parse(bytes) as ApiCatalog, bytes, output };
  };
  return { workspace, generate };
}

test('unpublished committed API revisions omit links without claiming working-tree changes', (t) => {
  const { workspace, generate } = fixture(t);
  const repo = join(workspace, 'llm-client');
  const published = git(repo, 'rev-parse', 'HEAD');
  write(join(repo, 'src/lib.rs'), 'pub struct LocalOnlyApi {}\n');
  git(repo, 'add', 'src/lib.rs');
  git(repo, 'commit', '-qm', 'unpublished API change');
  const revision = git(repo, 'rev-parse', 'HEAD');
  assert.notEqual(revision, published);
  for (const mode of ['committed', 'working-tree'] as const) {
    const current = generate(mode).catalog;
    const metadata = current.repositories.find((item) => item.sdk === 'llm');
    assert.equal(metadata?.revision, revision);
    assert.equal(metadata?.published, false);
    assert.equal(metadata?.dirty, false, 'a local commit is not a dirty file');
    const entry = current.entries.find((item) => item.name === 'LocalOnlyApi');
    assert.equal(entry?.sourceState, 'unpublished');
    assert.equal(entry?.url, undefined);
  }

  write(join(repo, 'src/lib.rs'), 'pub struct WorkingApi {}\n');
  const changed = generate('working-tree').catalog;
  assert.equal(changed.entries.find((item) => item.name === 'WorkingApi')?.sourceState, 'working-tree');
  assert.equal(changed.repositories.find((item) => item.sdk === 'llm')?.dirty, true);
  git(repo, 'restore', 'src/lib.rs');
  git(repo, 'update-ref', 'refs/remotes/origin/main', 'HEAD');
  const available = generate('committed').catalog;
  assert.equal(available.repositories.find((item) => item.sdk === 'llm')?.published, true);
  assert.equal(available.entries.find((item) => item.name === 'LocalOnlyApi')?.sourceState, 'committed');
  assert.equal(available.entries.find((item) => item.name === 'LocalOnlyApi')?.url,
    `https://github.com/lingxi-coder/llm-client/blob/${revision}/src/lib.rs#L1`);

  // The bridge uses its last source commit; publication of a descendant branch
  // is enough even when HEAD contains a later unpushed documentation commit.
  const bridge = join(workspace, 'lingxi-app');
  const bridgeRevision = git(bridge, 'rev-parse', 'HEAD');
  write(join(bridge, 'README.md'), 'published descendant\n');
  git(bridge, 'add', 'README.md');
  git(bridge, 'commit', '-qm', 'published documentation');
  git(bridge, 'update-ref', 'refs/remotes/origin/main', 'HEAD');
  write(join(bridge, 'README.md'), 'unpublished descendant\n');
  git(bridge, 'add', 'README.md');
  git(bridge, 'commit', '-qm', 'unpublished documentation');
  const bridgeCatalog = generate('committed').catalog;
  assert.equal(bridgeCatalog.repositories.find((item) => item.sdk === 'bridge')?.revision, bridgeRevision);
  assert.equal(bridgeCatalog.repositories.find((item) => item.sdk === 'bridge')?.published, true);
  assert.ok(bridgeCatalog.entries.filter((item) => item.sdk === 'bridge').every((item) => item.url));
});

test('a repository without known origin branches does not invent published source links', (t) => {
  const { workspace, generate } = fixture(t);
  git(join(workspace, 'mobile-linux-runtime'), 'update-ref', '-d', 'refs/remotes/origin/main');
  const current = generate('committed').catalog;
  const metadata = current.repositories.find((item) => item.sdk === 'mobile');
  assert.equal(metadata?.published, false);
  assert.equal(metadata?.dirty, false);
  assert.ok(current.entries.filter((item) => item.sdk === 'mobile').every((item) => item.sourceState === 'unpublished' && !item.url));
});

test('committed mode reads HEAD while working-tree mode tracks changed, staged, new and deleted UTF-8 inputs', (t) => {
  const { workspace, generate } = fixture(t);
  const repo = join(workspace, 'llm-client');
  const source = join(repo, 'src/lib.rs');
  const baseline = generate('committed').catalog;
  const baselineRepo = baseline.repositories.find((item) => item.sdk === 'llm');
  assert.ok(baselineRepo);
  assert.equal(baselineRepo.dirty, false);
  assert.ok(baseline.entries.every((entry) => entry.sourceState === 'committed' && entry.url));
  const defaultOutput = join(workspace, 'api-default.json');
  const defaultMode = spawnSync('python3', [script, '--workspace', workspace, '--output', defaultOutput], { encoding: 'utf8' });
  assert.equal(defaultMode.status, 0, defaultMode.stderr);
  assert.deepEqual(JSON.parse(readFileSync(defaultOutput, 'utf8')), baseline, 'direct CLI defaults to committed sources');

  // A line-only change invalidates a GitHub line link even when the declaration
  // signature is unchanged; unrelated inputs do not make this SDK dirty.
  write(source, '\n\npub struct LlmBase {}\n');
  write(join(repo, 'notes.md'), 'Not an API source.\n');
  let current = generate('working-tree').catalog;
  let entry = current.entries.find((item) => item.name === 'LlmBase');
  assert.equal(entry?.line, 3);
  assert.equal(entry?.sourceState, 'working-tree');
  assert.equal(entry?.url, undefined);
  assert.equal(current.repositories.find((item) => item.sdk === 'llm')?.dirty, true);
  assert.notEqual(current.repositories.find((item) => item.sdk === 'llm')?.treeFingerprint, baselineRepo.treeFingerprint);
  assert.deepEqual(generate('committed').catalog, baseline);

  git(repo, 'add', 'src/lib.rs');
  assert.deepEqual(generate('working-tree').catalog, current, 'staging must not change a content snapshot');
  write(join(repo, 'src/new.rs'), 'pub struct NewApi {}\n');
  write(join(repo, 'src/ignored.rs'), 'pub struct IgnoredApi {}\n');
  write(join(repo, 'src/tests/fixture.rs'), 'pub struct ExcludedApi {}\n');
  writeFileSync(join(repo, 'src/binary.rs'), Buffer.from([0xff, 0xfe, 0x00]));
  current = generate('working-tree').catalog;
  assert.equal(current.entries.find((item) => item.name === 'NewApi')?.sourceState, 'working-tree');
  assert.equal(current.entries.find((item) => item.name === 'NewApi')?.url, undefined);
  assert.ok(!current.entries.some((item) => ['IgnoredApi', 'ExcludedApi'].includes(item.name)));
  assert.deepEqual(generate('committed').catalog, baseline);

  rmSync(source);
  current = generate('working-tree').catalog;
  assert.ok(!current.entries.some((item) => item.name === 'LlmBase'));
  assert.deepEqual(generate('committed').catalog, baseline, 'deleted files remain in the committed mode');
  write(source, 'pub struct LlmBase {}\n');
  rmSync(join(repo, 'src/new.rs'));
  current = generate('working-tree').catalog;
  entry = current.entries.find((item) => item.name === 'LlmBase');
  assert.equal(entry?.sourceState, 'committed');
  assert.equal(entry?.url, `${baselineRepo.sourceUrl}/blob/${baselineRepo.revision}/src/lib.rs#L1`);
  assert.equal(current.repositories.find((item) => item.sdk === 'llm')?.dirty, false, 'restoring HEAD bytes clears input dirtiness even with staged changes');
  assert.equal(current.repositories.find((item) => item.sdk === 'llm')?.treeFingerprint, baselineRepo.treeFingerprint);
});

test('product dependency revision and deterministic checks stay independent of SDK input fingerprints', (t) => {
  const { workspace, generate } = fixture(t);
  const first = generate('working-tree');
  const second = generate('working-tree');
  assert.equal(first.bytes, second.bytes);
  assert.ok(!first.bytes.includes(workspace), 'catalog must not encode absolute workspace paths');
  const repo = first.catalog.repositories.find((item) => item.sdk === 'llm');
  assert.ok(repo);
  assert.equal(repo.productPinnedRevision, 'a'.repeat(40));
  assert.notEqual(repo.productPinnedRevision, repo.revision);
  write(join(workspace, 'lingxi-app/Cargo.toml'), `[workspace.dependencies]\nlingxi-llm-client = { git = "https://github.com/lingxi-coder/llm-client.git", rev = "${repo.revision}" }\n`);
  const updated = generate('working-tree');
  assert.equal(updated.catalog.repositories.find((item) => item.sdk === 'llm')?.productPinnedRevision, repo.revision);
  assert.equal(updated.catalog.repositories.find((item) => item.sdk === 'llm')?.treeFingerprint, repo.treeFingerprint);
  const check = spawnSync('python3', [script, '--workspace', workspace, '--output', updated.output, '--source-mode', 'working-tree', '--check'], { encoding: 'utf8' });
  assert.equal(check.status, 0, check.stderr);
  write(join(workspace, 'llm-client/src/lib.rs'), 'pub struct ChangedApi {}\n');
  const stale = spawnSync('python3', [script, '--workspace', workspace, '--output', updated.output, '--source-mode', 'working-tree', '--check'], { encoding: 'utf8' });
  assert.notEqual(stale.status, 0);
  assert.match(stale.stderr, /API catalog is stale/);
});

test('Bridge class and Kotlin facade extraction omit private methods and function-local variables', () => {
  const bridge = extract(`
export class Client {
  private secret(): void {}
  protected internal(): void {}
  /** Public connection. */
  async connect(
    config: Options,
  ): Promise<Result> {
    accidentalCall();
  }
  get status(): string { return 'ready'; }
}
`, 'typescript');
  assert.deepEqual(bridge.map(([name]) => name), ['Client', 'Client.connect', 'Client.status']);
  assert.equal(bridge[1][3], 'Public connection.');
  const mobile = extract(`
class Runtime {
  suspend fun boot(): Status {
    val local = 1
    return Status(local)
  }
  private fun hidden() {
    val privateLocal = 2
  }
}
`, 'kotlin');
  assert.deepEqual(mobile.map(([name]) => name), ['Runtime', 'Runtime.boot']);
});

test('block summaries belong only to the immediately preceding declaration comment', () => {
  const declarations = extract(`
/** Bridge module overview. */
import { EventEmitter } from 'node:events';

/** The first independent contract. */
export interface First {}

/**
 * The per-app capability level request_access asks for.
 * This is the second independent contract.
 */
export type AccessTierDto = 'workspace' | 'device';

export interface WithoutDocs {}

/** A comment for the private helper. */
function helper(): void {}
export interface AfterHelper {}

/** Earlier documentation. */
// An ordinary comment separates the item from the documentation.
export interface AfterComment {}
`, 'typescript');
  const docs = new Map(declarations.map(([name, , , comment]) => [name, comment]));
  assert.equal(docs.get('First'), 'The first independent contract.');
  assert.equal(docs.get('AccessTierDto'), 'The per-app capability level request_access asks for. This is the second independent contract.');
  assert.equal(docs.get('WithoutDocs'), '');
  assert.equal(docs.get('AfterHelper'), '');
  assert.equal(docs.get('AfterComment'), '');
  const entry = catalog.entries.find((item) => item.sdk === 'bridge' && item.name === 'AccessTierDto');
  assert.ok(entry);
  assert.match(entry.summary, /per-app capability level/);
  assert.doesNotMatch(entry.summary, /LingXi bridge wire-protocol types/);
});
