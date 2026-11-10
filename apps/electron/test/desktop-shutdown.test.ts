import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { setImmediate } from 'node:timers/promises';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';

// Execute the entry point's actual shutdown aggregation without importing
// Electron or starting the app. No parallel shutdown implementation is tested.
const source = ts.createSourceFile('index.ts', readFileSync(new URL('../src/main/index.ts', import.meta.url), 'utf8'), ts.ScriptTarget.Latest, true);
let shutdown: ts.CallExpression | undefined;
function findShutdown(node: ts.Node): void {
  if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === 'finally'
    && node.arguments.some(argument => ts.isArrowFunction(argument) && argument.body.getText(source).includes('app.quit()'))) shutdown = node;
  ts.forEachChild(node, findShutdown);
}
findShutdown(source);
assert.ok(shutdown, 'the real Desktop shutdown completion must be exercised');
const script = ts.transpileModule(`(${shutdown.getText(source)})`, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;

test('a failed shutdown phase cannot quit before delayed provider cleanup settles', async t => {
  let release!: () => void;
  const providerDrain = new Promise<void>(resolve => { release = resolve; });
  let releaseDiagnostics!: () => void;
  const diagnosticsDrain = new Promise<void>(resolve => { releaseDiagnostics = resolve; });
  let flushes = 0;
  let quits = 0;
  const events: Array<{ phase: string; status: string }> = [];
  const completion: Promise<void> = runInNewContext(script, {
    shutdownPhases: [
      ['provider_accounts', () => providerDrain],
      ['audio', async () => { throw new Error('audio cleanup failed'); }],
      ['bridge', async () => {}],
    ],
    shutdownDiagnostics: {
      add: (_level: string, _source: string, value: string) => events.push(JSON.parse(value)),
      flush: () => { flushes++; return diagnosticsDrain; },
    },
    app: { quit: () => { quits++; } },
  });
  // Observe a regression's rejection too, so the ordering assertion identifies
  // premature quit rather than leaving an unhandled rejected promise.
  const settled = completion.catch(() => {});
  t.after(async () => { release(); releaseDiagnostics(); await settled; });
  await setImmediate();
  assert.ok(events.some(event => event.phase === 'audio' && event.status === 'failed'));
  assert.ok(events.some(event => event.phase === 'bridge' && event.status === 'completed'));
  assert.equal(quits, 0, 'failed audio cleanup must still wait for the provider lock drain');
  assert.equal(flushes, 0, 'diagnostics flush starts after all shutdown producers settle');
  release();
  await setImmediate();
  assert.equal(flushes, 1);
  assert.equal(quits, 0, 'quit waits for the queued shutdown diagnostics');
  releaseDiagnostics();
  await completion;
  assert.ok(events.some(event => event.phase === 'provider_accounts' && event.status === 'completed'));
  assert.equal(quits, 1);
});
