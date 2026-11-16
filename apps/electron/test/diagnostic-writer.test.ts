import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { DiagnosticFileWriter } from '../src/main/diagnostic-writer.js';

test('ordinary appends never construct the retained tail, and rotation uses only dequeued batches', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'diagnostic-writer-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = join(directory, 'desktop.jsonl');
  const writer = new DiagnosticFileWriter(path, 'live', 200);
  // Count the allocation at the retained array, without replacing file IO or
  // scheduling: ordinary append batches must not join this whole history.
  const tail = (writer as unknown as { tail: string[] }).tail;
  let joins = 0;
  tail.join = function (separator?: string) { joins++; return Array.prototype.join.call(this, separator); };
  for (let index = 0; index < 3; index++) {
    writer.add(`append-${index}\n`);
    await writer.flush();
  }
  assert.equal(joins, 0);
  assert.equal(readFileSync(path, 'utf8'), 'append-0\nappend-1\nappend-2\n');
  writeFileSync(path, 'x'.repeat(2_000_000));
  writer.add('rotation-batch\n');
  const completion = writer.flush();
  for (let index = 0; index < 100; index++) writer.add(`future-${index}\n`);
  await completion;
  assert.equal(joins, 1);
  assert.equal(readFileSync(path, 'utf8'), ['append-0', 'append-1', 'append-2', 'rotation-batch',
    ...Array.from({ length: 100 }, (_, index) => `future-${index}`), ''].join('\n'));
  assert.equal(readFileSync(`${path}.1`, 'utf8').length, 2_000_000);
});

test('rotation retains the backlog drop marker and each queued line once', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'diagnostic-writer-drop-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = join(directory, 'desktop.jsonl');
  writeFileSync(path, 'x'.repeat(2_000_000));
  const writer = new DiagnosticFileWriter(path, 'live', 200, ['seed-0\n', 'seed-1\n']);
  for (let index = 0; index < 1_026; index++) writer.add(`queued-${index}\n`);
  await writer.flush();
  const lines = readFileSync(path, 'utf8').trim().split('\n');
  assert.deepEqual(lines.slice(0, 2), ['seed-0', 'seed-1']);
  assert.deepEqual(JSON.parse(JSON.parse(lines[2]).message), { event: 'diagnostic_backlog_dropped', queue: 'live', count: 2 });
  assert.deepEqual(lines.slice(3), Array.from({ length: 1_024 }, (_, index) => `queued-${index + 2}`));
});
