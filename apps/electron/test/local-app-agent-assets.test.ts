import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { LocalAppChatAssetError, LocalAppChatAssetJournal } from '../src/main/local-apps/agentChatAssets.ts';

const payload = (value: Buffer, id = 'user-1') => ({
  id, role: 'user', content: [
    { type: 'text', text: 'Read this private test fixture.' },
    { type: 'document', source: { type: 'data', value: value.toString('base64'), mimeType: 'text/plain' }, metadata: { filename: 'fixture.txt' } },
  ],
});

async function withRoot(run: (root: string) => Promise<void>) {
  const root = await fs.mkdtemp(join(tmpdir(), 'local-app-agent-assets-'));
  try { await run(root); } finally { await fs.rm(root, { recursive: true, force: true }); }
}

test('journal stores content-addressed private bytes and rehydrates the exact public message', async () => {
  await withRoot(async root => {
    const bytes = Buffer.from('stationery count 3');
    const journal = new LocalAppChatAssetJournal(join(root, 'chat'));
    const original = payload(bytes);
    const stored = await journal.storeMessage(original);
    assert.deepEqual(stored.assetHashes, [createHash('sha256').update(bytes).digest('hex')]);
    assert.equal(stored.newMessageBytes, bytes.length);
    assert.equal(JSON.stringify(stored.message).includes(bytes.toString('base64')), false);
    assert.deepEqual(journal.canonicalizeMessage(original), stored.message);
    assert.notDeepEqual(journal.canonicalizeMessage(payload(Buffer.from('different'))), stored.message);
    assert.deepEqual(await journal.rehydrateMessage(stored.message), original);
    const assets = await fs.readdir(join(root, 'chat', 'assets'));
    assert.deepEqual(assets, [`${stored.assetHashes[0]}.bin`]);
    if (process.platform !== 'win32') assert.equal((await fs.stat(join(root, 'chat', 'assets', assets[0]!))).mode & 0o777, 0o600);
  });
});

test('asset storage deduplicates identical bytes and rejects aggregate chat storage overflow', async () => {
  await withRoot(async root => {
    const journal = new LocalAppChatAssetJournal(join(root, 'chat'), { maxChatAssetBytes: 5 });
    const first = await journal.storeMessage(payload(Buffer.from('abc'), 'user-1'));
    const duplicate = await journal.storeMessage(payload(Buffer.from('abc'), 'user-2'));
    assert.deepEqual(duplicate.assetHashes, first.assetHashes);
    assert.equal((await fs.readdir(join(root, 'chat', 'assets'))).length, 1);
    await assert.rejects(journal.storeMessage(payload(Buffer.from('def'), 'user-3')), error => error instanceof LocalAppChatAssetError && error.code === 'chat_asset_limit');
  });
});

test('new message file count, file size, aggregate bytes and inline source kind are bounded', async () => {
  await withRoot(async root => {
    const journal = new LocalAppChatAssetJournal(join(root, 'chat'), {
      maxFilesPerMessage: 1, maxFileBytes: 4, maxNewMessageBytes: 4,
    });
    const withTwoParts = { ...payload(Buffer.from('a')), content: [
      { type: 'image', source: { type: 'data', value: Buffer.from('a').toString('base64'), mimeType: 'image/png' } },
      { type: 'document', source: { type: 'data', value: Buffer.from('b').toString('base64'), mimeType: 'text/plain' } },
    ] };
    await assert.rejects(journal.storeMessage(withTwoParts), error => error instanceof LocalAppChatAssetError && error.code === 'attachment_limit');
    await assert.rejects(journal.storeMessage(payload(Buffer.from('12345'))), error => error instanceof LocalAppChatAssetError && error.code === 'attachment_too_large');
    await assert.rejects(journal.storeMessage({ ...payload(Buffer.from('a')), content: [
      { type: 'document', source: { type: 'url', url: 'https://example.test/private.txt' } },
    ] }), error => error instanceof LocalAppChatAssetError && error.code === 'unsupported_attachment_source');
    await assert.rejects(journal.storeMessage({ ...payload(Buffer.from('a')), content: [
      { type: 'document', source: { type: 'data', value: 'not-base64', mimeType: 'text/plain' } },
    ] }), error => error instanceof LocalAppChatAssetError && error.code === 'invalid_attachment');
  });
});

test('linear bounded Base64 validation accepts realistic 8 MiB and maximum 25 MiB files', async () => {
  await withRoot(async root => {
    for (const [index, size] of [8, 25].entries()) {
      const bytes = Buffer.alloc(size * 1024 * 1024, index + 3);
      const journal = new LocalAppChatAssetJournal(join(root, `chat-${size}`));
      const stored = await journal.storeMessage(payload(bytes, `user-${size}`));
      assert.equal(stored.newMessageBytes, bytes.length);
      assert.equal(stored.assetHashes[0], createHash('sha256').update(bytes).digest('hex'));
    }
  });
});

test('history rehydration enforces both a single-message and aggregate page byte limit without truncating', async () => {
  await withRoot(async root => {
    const journal = new LocalAppChatAssetJournal(join(root, 'chat'), { maxHistoryResponseBytes: 180 });
    const stored = await journal.storeMessage(payload(Buffer.alloc(128, 7)));
    await assert.rejects(journal.rehydrateMessage(stored.message), error => error instanceof LocalAppChatAssetError && error.code === 'history_message_too_large');
  });
  await withRoot(async root => {
    const journal = new LocalAppChatAssetJournal(join(root, 'chat'), { maxHistoryResponseBytes: 250 });
    const first = await journal.storeMessage(payload(Buffer.from('one'), 'user-1'));
    const second = await journal.storeMessage(payload(Buffer.from('two'), 'user-2'));
    await assert.rejects(journal.rehydrateMessages([first.message, second.message]), error => error instanceof LocalAppChatAssetError && error.code === 'history_page_too_large');
  });
});

test('rehydration rejects symlink and digest-tampered assets', async () => {
  await withRoot(async root => {
    const journal = new LocalAppChatAssetJournal(join(root, 'chat'));
    const bytes = Buffer.from('private file');
    const stored = await journal.storeMessage(payload(bytes));
    const assetPath = join(root, 'chat', 'assets', `${stored.assetHashes[0]}.bin`);
    const outside = join(root, 'outside.bin');
    await fs.writeFile(outside, bytes);
    await fs.rm(assetPath);
    await fs.symlink(outside, assetPath);
    await assert.rejects(journal.rehydrateMessage(stored.message), error => error instanceof LocalAppChatAssetError && error.code === 'asset_storage_invalid');
    await fs.rm(assetPath);
    await fs.writeFile(assetPath, 'tampered');
    await assert.rejects(journal.rehydrateMessage(stored.message), error => error instanceof LocalAppChatAssetError && error.code === 'asset_storage_invalid');
  });
});

test('pruning removes only assets that no canonical journal message references', async () => {
  await withRoot(async root => {
    const journal = new LocalAppChatAssetJournal(join(root, 'chat'));
    const first = await journal.storeMessage(payload(Buffer.from('first'), 'user-1'));
    await journal.storeMessage(payload(Buffer.from('second'), 'user-2'));
    await journal.pruneUnreferenced([first.message]);
    assert.deepEqual(await fs.readdir(join(root, 'chat', 'assets')), [`${first.assetHashes[0]}.bin`]);
    assert.deepEqual(await journal.rehydrateMessage(first.message), payload(Buffer.from('first'), 'user-1'));
  });
});
