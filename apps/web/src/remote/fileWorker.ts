/** OPFS synchronous access is confined to a worker: flush before acknowledging
 * each transfer window without rewriting/copying earlier downloaded bytes. */
import { hashFile, validateFileOffer, type FileOffer } from '@lingxi/bridge-client/remote';
interface Access {
  read(bytes: Uint8Array, options: { at: number }): number;
  write(bytes: Uint8Array, options: { at: number }): number;
  getSize(): number;
  truncate(size: number): void;
  flush(): void;
  close(): void;
}
let data: Access | undefined;
let checkpoint: Access | undefined;
let offer: FileOffer | undefined;
let offset = 0;
let durable = 0;
let directory: FileSystemDirectoryHandle | undefined;
let handle: FileSystemFileHandle | undefined;
async function access(handle: FileSystemFileHandle): Promise<Access> {
  return (handle as unknown as { createSyncAccessHandle(): Promise<Access> }).createSyncAccessHandle();
}
function release(): void { data?.close(); checkpoint?.close(); data = checkpoint = undefined; }
let queue = Promise.resolve();
globalThis.onmessage = event => {
  const message = event.data as { seq: number; t: string; scope?: string; offer?: FileOffer; offset?: number; bytes?: Uint8Array };
  queue = queue.then(async () => {
    try {
      let result: unknown;
      switch (message.t) {
        case 'open': {
          release();
          offer = validateFileOffer(message.offer);
          if (!/^[0-9a-f]{64}$/.test(message.scope ?? '')) throw new Error('invalid file scope');
          directory = await (await navigator.storage.getDirectory()).getDirectoryHandle(`rc-files-${message.scope}`, { create: true });
          handle = await directory.getFileHandle(offer.transfer_id, { create: true });
          data = await access(handle);
          checkpoint = await access(await directory.getFileHandle(`${offer.transfer_id}.offset`, { create: true }));
          const bytes = new Uint8Array(8);
          offset = checkpoint.getSize() === 8 && checkpoint.read(bytes, { at: 0 }) === 8 ? Number(new DataView(bytes.buffer).getBigUint64(0)) : 0;
          if (!Number.isSafeInteger(offset) || offset < 0 || offset > offer.size || offset > data.getSize()) throw new Error('invalid file checkpoint');
          data.truncate(offset); durable = offset;
          result = offset;
          break;
        }
        case 'write': {
          const bytes = message.bytes;
          if (!data || !checkpoint || !offer || !bytes?.length || message.offset !== offset || offset + bytes.length > offer.size) throw new Error('invalid file chunk');
          let written = 0;
          while (written < bytes.length) {
            const n = data.write(bytes.subarray(written), { at: offset + written });
            if (!n) throw new Error('short file write');
            written += n;
          }
          offset += bytes.length; result = offset;
          break;
        }
        case 'checkpoint': {
          if (!data || !checkpoint) throw new Error('file storage closed');
          data.flush();
          const saved = new Uint8Array(8); new DataView(saved.buffer).setBigUint64(0, BigInt(offset));
          if (checkpoint.write(saved, { at: 0 }) !== 8) throw new Error('short checkpoint write');
          checkpoint.flush(); durable = offset; result = durable;
          break;
        }
        case 'finish': {
          if (!data || !offer || durable !== offer.size) throw new Error('incomplete file');
          const file = data;
          const hash = await hashFile({ size: offer.size, read: async (at, length) => {
            const bytes = new Uint8Array(length);
            const count = file.read(bytes, { at });
            return bytes.subarray(0, count);
          } });
          if (hash !== offer.sha256) {
            data.truncate(0); data.flush(); checkpoint!.truncate(0); checkpoint!.flush(); offset = 0;
            throw new Error('file checksum mismatch');
          }
          release();
          result = await handle!.getFile(); // disk-backed Blob, never assembled as an array of chunks.
          break;
        }
        case 'cancel':
          release();
          if (directory && offer) {
            await directory.removeEntry(offer.transfer_id);
            await directory.removeEntry(`${offer.transfer_id}.offset`);
          }
          break;
        case 'close': release(); break;
        default: throw new Error('invalid file worker operation');
      }
      globalThis.postMessage({ seq: message.seq, result });
    } catch (error) {
      release();
      globalThis.postMessage({ seq: message.seq, error: error instanceof Error ? error.message : String(error) });
    }
  });
};
