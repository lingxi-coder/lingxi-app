/** Durable, account/device/session-scoped attachment staging. No viewer-supplied
 * path is opened. A receipt becomes usable only after SHA-256 verification. */
import { createHash } from 'node:crypto';
import { mkdir, open, readFile, rename, rm, stat, readdir } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { validateFileOffer, type FileOffer, type FileSink, type FileReceipt, type FileSource } from '@lingxi/bridge-client/remote';

export class RemoteFileStore {
  private readonly active = new Set<string>();
  constructor(private readonly root: string) {}
  private directory(owner: string, session: string, id: string): string {
    if (!/^[0-9a-f]{32}$/.test(id)) throw new Error('invalid transfer id');
    return join(this.root, createHash('sha256').update(owner).update('\0').update(session).digest('hex'), id);
  }
  async accept(owner: string, raw: FileOffer, authorize: () => void): Promise<FileSink> {
    const offer = validateFileOffer(raw);
    const dir = this.directory(owner, offer.session_id, offer.transfer_id);
    if (this.active.has(dir)) throw new Error('transfer already active');
    this.active.add(dir);
    let handle: Awaited<ReturnType<typeof open>> | undefined;
    try {
      authorize();
      await durableDirectory(dir);
      const saved = await readJson(join(dir, 'offer.json'));
      if (saved !== undefined && JSON.stringify(validateFileOffer(saved)) !== JSON.stringify(offer)) throw new Error('transfer metadata changed');
      if (saved === undefined) await atomicJson(dir, 'offer.json', offer);
      let receipt: FileReceipt | undefined;
      try { receipt = await this.receipt(owner, offer.session_id, offer.transfer_id); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
      // Recover a crash between the final rename and receipt publication.
      if (!receipt) {
        const final = await stat(join(dir, 'data')).catch((error: NodeJS.ErrnoException) => { if (error.code === 'ENOENT') return undefined; throw error; });
        if (final) {
          if (final.size !== offer.size || await digest(join(dir, 'data')) !== offer.sha256) throw new Error('invalid final file');
          receipt = { ...offer, ref: offer.transfer_id };
          await atomicJson(dir, 'receipt.json', receipt);
        }
      }
      let offset = receipt ? offer.size : (await readJson(join(dir, 'checkpoint.json')) as { offset?: number } | undefined)?.offset ?? 0;
      if (!Number.isSafeInteger(offset) || offset < 0 || offset > offer.size) throw new Error('invalid saved checkpoint');
      if (!receipt) {
        handle = await open(join(dir, 'data.part'), 'a+', 0o600);
        if ((await handle.stat()).size < offset) throw new Error('incomplete checkpoint');
        await handle.truncate(offset); // Discard bytes written before a lost checkpoint.
      }
      let received = offset;
      const checkpoint = async (): Promise<void> => {
        if (received === offset) return;
        await handle!.sync();
        await atomicJson(dir, 'checkpoint.json', { offset: received });
        offset = received;
      };
      let done = false;
      const release = async (): Promise<void> => {
        if (done) return;
        done = true;
        try { if (handle) await checkpoint(); } finally {
          try { await handle?.close(); } finally { this.active.delete(dir); }
        }
      };
      return {
        get offset() { return offset; },
        write: async (at, bytes) => {
          authorize();
          if (done || receipt || at !== received || bytes.length === 0 || received + bytes.length > offer.size) throw new Error('invalid file chunk');
          // O_APPEND permits only sequential writes; do not trust a remote offset as a seek.
          let written = 0;
          while (written < bytes.length) {
            const result = await handle!.write(bytes, written, bytes.length - written);
            if (result.bytesWritten === 0) throw new Error('short file write');
            written += result.bytesWritten;
          }
          received += bytes.length;
        },
        checkpoint: async () => { authorize(); await checkpoint(); },
        finish: async () => {
          authorize();
          if (done) throw new Error('transfer closed');
          if (receipt) { await release(); return receipt; }
          await checkpoint();
          if (offset !== offer.size) throw new Error('incomplete file');
          await handle!.sync();
          await handle!.close(); handle = undefined;
          const path = join(dir, 'data.part');
          const hash = await digest(path);
          if (hash !== offer.sha256) {
            await release();
            await rm(dir, { recursive: true, force: true });
            throw new Error('file checksum mismatch');
          }
          authorize();
          await rename(path, join(dir, 'data'));
          const result: FileReceipt = { ...offer, ref: offer.transfer_id };
          await atomicJson(dir, 'receipt.json', result);
          await release();
          return result;
        },
        close: release,
        cancel: async () => { await release(); if (!receipt) await rm(dir, { recursive: true, force: true }); },
      };
    } catch (error) {
      await handle?.close().catch(() => undefined);
      this.active.delete(dir);
      throw error;
    }
  }
  async receipt(owner: string, session: string, id: string): Promise<FileReceipt> {
    const dir = this.directory(owner, session, id);
    const raw = await readJson(join(dir, 'receipt.json'));
    if (raw === undefined) throw Object.assign(new Error('file not found'), { code: 'ENOENT' });
    const metadata = validateFileOffer(raw);
    if (metadata.session_id !== session || metadata.transfer_id !== id || (await stat(join(dir, 'data'))).size !== metadata.size) throw new Error('invalid receipt');
    return { ...metadata, ref: id };
  }
  async list(owner: string, session: string): Promise<FileReceipt[]> {
    const parent = this.directory(owner, session, '0'.repeat(32));
    const dirs = await readdir(join(parent, '..')).catch((error: NodeJS.ErrnoException) => { if (error.code === 'ENOENT') return []; throw error; });
    const result: FileReceipt[] = [];
    for (const id of dirs) {
      if (!/^[0-9a-f]{32}$/.test(id)) continue;
      const receipt = await this.receipt(owner, session, id).catch(() => undefined);
      if (receipt) result.push(receipt);
    }
    return result;
  }
  async promptReference(owner: string, session: string, id: string): Promise<string> {
    const receipt = await this.receipt(owner, session, id);
    return `Attached file ${JSON.stringify(receipt.name)} (${receipt.size} bytes, SHA-256 ${receipt.sha256}). Local path: ${JSON.stringify(join(this.directory(owner, session, id), 'data'))}`;
  }
  async source(owner: string, session: string, id: string): Promise<{ offer: FileReceipt; source: FileSource }> {
    const offer = await this.receipt(owner, session, id);
    const path = join(this.directory(owner, session, id), 'data');
    return { offer, source: {
      size: offer.size,
      read: async (offset, length) => {
        const file = await open(path, 'r');
        try {
          const buffer = new Uint8Array(Math.min(length, offer.size - offset));
          let read = 0;
          while (read < buffer.length) {
            const result = await file.read(buffer, read, buffer.length - read, offset + read);
            if (!result.bytesRead) break;
            read += result.bytesRead;
          }
          return buffer.subarray(0, read);
        } finally { await file.close(); }
      },
    } };
  }
}
async function readJson(path: string): Promise<unknown> {
  try { return JSON.parse(await readFile(path, 'utf8')); }
  catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; }
}
async function atomicJson(dir: string, name: string, value: unknown): Promise<void> {
  const temp = join(dir, `${name}.tmp`);
  const file = await open(temp, 'w', 0o600);
  try { await file.writeFile(JSON.stringify(value)); await file.sync(); } finally { await file.close(); }
  await rename(temp, join(dir, name));
  const directory = await open(dir, 'r');
  try { await directory.sync(); } finally { await directory.close(); }
}
async function digest(path: string): Promise<string> {
  const hash = createHash('sha256');
  const file = await open(path, 'r');
  try { for await (const chunk of file.createReadStream({ autoClose: false })) hash.update(chunk); }
  finally { await file.close(); }
  return hash.digest('hex');
}

async function durableDirectory(path: string): Promise<void> {
  const created = await mkdir(path, { recursive: true, mode: 0o700 });
  if (!created) return;
  const stop = dirname(resolve(created));
  for (let current = resolve(path);; current = dirname(current)) {
    const directory = await open(current, 'r');
    try { await directory.sync(); } finally { await directory.close(); }
    if (current === stop) break;
  }
}
