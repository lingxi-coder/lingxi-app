import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { FileRefusal, transferFiles, type TransferLimits } from '../src/main/bots/taskFiles.js';

const TASK = 'Task000000000000000001';

function setup(t: { after(fn: () => void): void }) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'lingxi-task-files-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const from = join(root, 'from');
  const to = join(root, 'to');
  const outside = join(root, 'outside');
  for (const directory of [from, to, outside]) mkdirSync(directory);
  const write = (path: string, contents: string | Buffer): string => {
    mkdirSync(join(path, '..'), { recursive: true });
    writeFileSync(path, contents);
    return path;
  };
  return { root, from, to, outside, write };
}

const sha256 = (contents: string | Buffer): string => createHash('sha256').update(contents).digest('hex');

async function refused(promise: Promise<unknown>, message: RegExp): Promise<void> {
  await assert.rejects(promise, (error: Error) => {
    assert.ok(error instanceof FileRefusal, `a refusal, not ${error.name}: ${error.message}`);
    assert.match(error.message, message);
    return true;
  });
}

test('a file is copied into the inbox of the other workspace, hashed, under its own name', async (t) => {
  const s = setup(t);
  s.write(join(s.from, 'report.csv'), 'a,b\n1,2\n');
  s.write(join(s.from, 'sub', 'dir', 'notes.txt'), 'notes');
  const copies = await transferFiles(s.from, ['report.csv', join(s.from, 'sub', 'dir', 'notes.txt'), './sub/dir/notes.txt'], s.to, TASK);
  assert.deepEqual(copies, [
    { name: 'report.csv', path: `inbox/${TASK}/report.csv`, size: 8, sha256: sha256('a,b\n1,2\n') },
    { name: 'notes.txt', path: `inbox/${TASK}/notes.txt`, size: 5, sha256: sha256('notes') },
    { name: 'notes (2).txt', path: `inbox/${TASK}/notes (2).txt`, size: 5, sha256: sha256('notes') },
  ]);
  assert.equal(readFileSync(join(s.to, 'inbox', TASK, 'report.csv'), 'utf8'), 'a,b\n1,2\n');
  assert.equal(statSync(join(s.to, 'inbox', TASK, 'report.csv')).mode & 0o777, 0o600);
  assert.equal(readFileSync(join(s.from, 'report.csv'), 'utf8'), 'a,b\n1,2\n', 'the original stays');
  const more = await transferFiles(s.from, ['report.csv'], s.to, TASK);
  assert.equal(more[0]!.name, 'report (2).csv', 'a name that is taken gets a number, also in a later call');
});

test('nothing outside the workspace can be named', async (t) => {
  const s = setup(t);
  s.write(join(s.outside, 'secret.txt'), 'secret');
  s.write(join(s.from, 'ok.txt'), 'ok');
  await refused(transferFiles(s.from, ['../outside/secret.txt'], s.to, TASK), /"\.\."/);
  await refused(transferFiles(s.from, ['sub/../ok.txt'], s.to, TASK), /"\.\."/);
  await refused(transferFiles(s.from, [`${s.from}/../outside/secret.txt`], s.to, TASK), /"\.\."/);
  await refused(transferFiles(s.from, [join(s.outside, 'secret.txt')], s.to, TASK), /outside your workspace/);
  await refused(transferFiles(s.from, ['/etc/hosts'], s.to, TASK), /outside your workspace/);
  await refused(transferFiles(s.from, [`${s.from}-sibling/ok.txt`], s.to, TASK), /outside your workspace/);
  await refused(transferFiles(s.from, ['', 'ok.txt'], s.to, TASK), /Name the file/);
  await refused(transferFiles(s.from, ['ok\0.txt'], s.to, TASK), /Name the file/);
  await refused(transferFiles(s.from, ['.'], s.to, TASK), /is not a file/);
  await refused(transferFiles(s.from, [s.from], s.to, TASK), /is not a file/);
  assert.equal(existsSync(join(s.to, 'inbox')), false, 'nothing was made in the other workspace');
});

test('a directory, a missing file and a file in a missing directory are refused', async (t) => {
  const s = setup(t);
  mkdirSync(join(s.from, 'folder'));
  s.write(join(s.from, 'file.txt'), 'x');
  await refused(transferFiles(s.from, ['folder'], s.to, TASK), /not a regular file/);
  await refused(transferFiles(s.from, ['missing.txt'], s.to, TASK), /does not exist/);
  await refused(transferFiles(s.from, ['nothere/file.txt'], s.to, TASK), /does not exist/);
  await refused(transferFiles(s.from, ['file.txt/below'], s.to, TASK), /not a regular file/);
});

test('a symbolic link is not followed, whether it is the file or a folder on the way to it', async (t) => {
  const s = setup(t);
  s.write(join(s.outside, 'secret.txt'), 'secret');
  s.write(join(s.from, 'real.txt'), 'real');
  symlinkSync(join(s.outside, 'secret.txt'), join(s.from, 'link.txt'));
  symlinkSync(join(s.from, 'real.txt'), join(s.from, 'inside.txt'));
  symlinkSync(s.outside, join(s.from, 'linked-folder'));
  await refused(transferFiles(s.from, ['link.txt'], s.to, TASK), /symbolic links are not copied/);
  await refused(transferFiles(s.from, ['inside.txt'], s.to, TASK), /symbolic links are not copied/);
  await refused(transferFiles(s.from, ['linked-folder/secret.txt'], s.to, TASK), /symbolic links are not copied/);
  assert.deepEqual(readdirSync(s.to), [], 'nothing was copied');
});

test('a workspace that is itself reached through a link can still be named by either spelling', async (t) => {
  const s = setup(t);
  const alias = join(s.root, 'alias');
  symlinkSync(s.from, alias);
  s.write(join(s.from, 'a.txt'), 'a');
  const copies = await transferFiles(alias, ['a.txt', join(alias, 'a.txt'), join(s.from, 'a.txt')], s.to, TASK);
  assert.equal(copies.length, 3);
  await refused(transferFiles(alias, [join(s.outside, 'x')], s.to, TASK), /outside your workspace/);
});

test('a file with a second name is not copied: it may really be somebody else\'s', async (t) => {
  const s = setup(t);
  s.write(join(s.outside, 'secret.txt'), 'secret');
  linkSync(join(s.outside, 'secret.txt'), join(s.from, 'hard.txt'));
  await refused(transferFiles(s.from, ['hard.txt'], s.to, TASK), /single name/);
  assert.deepEqual(readdirSync(s.to), []);
});

test('the size caps are kept: one file, all of them together, and how many', async (t) => {
  const s = setup(t);
  const limits: TransferLimits = { maxFiles: 3, maxFileBytes: 10, maxTotalBytes: 15 };
  s.write(join(s.from, 'big.bin'), Buffer.alloc(11));
  s.write(join(s.from, 'a.bin'), Buffer.alloc(8));
  s.write(join(s.from, 'b.bin'), Buffer.alloc(8));
  s.write(join(s.from, 'edge.bin'), Buffer.alloc(10));
  await refused(transferFiles(s.from, ['big.bin'], s.to, TASK, limits), /larger than/);
  assert.equal((await transferFiles(s.from, ['edge.bin'], s.to, TASK, limits))[0]!.size, 10, 'a file of exactly the cap is fine');
  await refused(transferFiles(s.from, ['a.bin', 'b.bin'], s.to, 'Task000000000000000002', limits), /in all/);
  await refused(transferFiles(s.from, ['a.bin', 'a.bin', 'a.bin', 'a.bin'], s.to, 'Task000000000000000003', limits), /at most 3 files/);
  assert.deepEqual(readdirSync(join(s.to, 'inbox')).sort(), [TASK], 'what was refused left nothing behind, not even its folder');
});

test('files are copied all or none', async (t) => {
  const s = setup(t);
  s.write(join(s.from, 'good.txt'), 'good');
  await refused(transferFiles(s.from, ['good.txt', 'missing.txt'], s.to, TASK), /does not exist/);
  assert.deepEqual(existsSync(join(s.to, 'inbox', TASK)) ? readdirSync(join(s.to, 'inbox', TASK)) : [], [], 'the first copy was taken back');
});

test('a link planted in the receiving workspace cannot send a copy elsewhere', async (t) => {
  const s = setup(t);
  s.write(join(s.from, 'a.txt'), 'a');
  symlinkSync(s.outside, join(s.to, 'inbox'));
  await refused(transferFiles(s.from, ['a.txt'], s.to, TASK), /in the way/);
  unlinkSync(join(s.to, 'inbox'));
  mkdirSync(join(s.to, 'inbox'));
  symlinkSync(s.outside, join(s.to, 'inbox', TASK));
  await refused(transferFiles(s.from, ['a.txt'], s.to, TASK), /in the way/);
  unlinkSync(join(s.to, 'inbox', TASK));
  mkdirSync(join(s.to, 'inbox', TASK));
  symlinkSync(join(s.outside, 'target.txt'), join(s.to, 'inbox', TASK, 'a.txt'));
  const copies = await transferFiles(s.from, ['a.txt'], s.to, TASK);
  assert.equal(copies[0]!.name, 'a (2).txt', 'a link in the way of the name is not written through');
  assert.deepEqual(readdirSync(s.outside), [], 'nothing reached the folder the links pointed at');
});

test('the name of a copy is one safe line', async (t) => {
  const s = setup(t);
  s.write(join(s.from, 'tab\there.txt'), 'x');
  const [copy] = await transferFiles(s.from, ['tab\there.txt'], s.to, TASK);
  assert.equal(copy!.name, 'tabhere.txt');
  s.write(join(s.from, '.hidden'), 'x');
  assert.equal((await transferFiles(s.from, ['.hidden'], s.to, TASK))[0]!.name, '.hidden');
});

test('a workspace that is gone is an error of the host, not a refusal of the Bot', async (t) => {
  const s = setup(t);
  s.write(join(s.from, 'a.txt'), 'a');
  rmSync(s.to, { recursive: true });
  await assert.rejects(transferFiles(s.from, ['a.txt'], s.to, TASK), (error: NodeJS.ErrnoException) => error.code === 'ENOENT');
  assert.deepEqual(await transferFiles(s.from, [], join(s.root, 'nowhere'), TASK), [], 'nothing to copy needs no workspace');
});
