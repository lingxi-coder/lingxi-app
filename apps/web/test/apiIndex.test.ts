import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
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
  url: string;
}

interface ApiRepository {
  sdk: string;
  revision: string;
  sourceUrl: string;
}

const catalog = JSON.parse(readFileSync(new URL('../public/api-index.json', import.meta.url), 'utf8')) as {
  generatedAt: string;
  repositories: ApiRepository[];
  entries: ApiEntry[];
};

test('SDK catalog has usable declarations and exact committed source links for every SDK', () => {
  assert.match(catalog.generatedAt, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/);
  assert.deepEqual(catalog.repositories.map((repo) => repo.sdk).sort(), ['bridge', 'harness', 'llm', 'mobile']);
  const repositories = new Map(catalog.repositories.map((repo) => [repo.sdk, repo]));
  const identities = new Set<string>();
  for (const repo of catalog.repositories) {
    assert.match(repo.revision, /^[a-f0-9]{40}$/);
    assert.match(repo.sourceUrl, /^https:\/\/github\.com\/lingxi-coder\/[\w-]+$/);
    assert.ok(catalog.entries.filter((entry) => entry.sdk === repo.sdk).length > 10, repo.sdk);
  }
  for (const entry of catalog.entries) {
    const repo = repositories.get(entry.sdk);
    assert.ok(repo, `unknown SDK: ${entry.sdk}`);
    assert.ok(entry.name && entry.kind && entry.signature, JSON.stringify(entry));
    assert.equal(typeof entry.summary, 'string');
    assert.ok(Number.isInteger(entry.line) && entry.line > 0);
    assert.equal(entry.url, `${repo.sourceUrl}/blob/${repo.revision}/${entry.file}#L${entry.line}`);
    assert.doesNotMatch(entry.file, /(^|\/)(tests?|fixtures?|vendor|snapshots|examples)(\/|\.)|(_tests?|test_support)\.(rs|ts)$/);
    assert.doesNotMatch(entry.signature, /^pub\s*\([^)]*\)|^(private|protected|internal)\b/);
    const identity = `${entry.sdk}:${entry.file}:${entry.line}:${entry.name}`;
    assert.equal(identities.has(identity), false, identity);
    identities.add(identity);
  }
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
  const script = fileURLToPath(new URL('../scripts/generate-api-index.py', import.meta.url));
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
