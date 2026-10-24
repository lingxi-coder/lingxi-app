import { createHash } from 'node:crypto';
import { inflateRawSync } from 'node:zlib';
import { promises as fs } from 'node:fs';
import { dirname, join } from 'node:path';

export interface SourceFile { path: string; sizeBytes: number; sha256: string; executable: boolean }
const MAX_ARCHIVE = 25 * 1024 * 1024, MAX_EXPANDED = 100 * 1024 * 1024;
export const sha256 = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex');
export function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') return `{${Object.keys(value).sort().map(key => JSON.stringify(key) + ':' + canonical((value as Record<string, unknown>)[key])).join(',')}}`;
  return JSON.stringify(value);
}
export function packagePath(path: string): void {
  if (!path || path !== path.normalize('NFC') || /[\\%?#:\x00-\x1f]/.test(path) || path.startsWith('/') || path.split('/').some(p => !p || p === '.' || p === '..')) throw new Error('Unsafe package path');
}
/** Decode a bounded ZIP only after matching every entry to the immutable inventory. */
export async function unpackSource(bytes: Buffer, inventory: SourceFile[], destination: string): Promise<void> {
  if (!bytes.length || bytes.length > MAX_ARCHIVE || inventory.length > 2000) throw new Error('Package exceeds limits');
  const files = new Map(inventory.map(file => [file.path, file]));
  if (files.size !== inventory.length || !files.has('lingxi-app.json')) throw new Error('Invalid package inventory');
  let eocd = -1;
  for (let i = bytes.length - 22; i >= Math.max(0, bytes.length - 65557); i--) if (bytes.readUInt32LE(i) === 0x06054b50 && i + 22 + bytes.readUInt16LE(i + 20) === bytes.length) { eocd = i; break; }
  if (eocd < 0 || bytes.readUInt16LE(eocd + 4) || bytes.readUInt16LE(eocd + 6) || bytes.readUInt16LE(eocd + 8) !== bytes.readUInt16LE(eocd + 10)) throw new Error('Unsupported ZIP');
  const count = bytes.readUInt16LE(eocd + 10), centralSize = bytes.readUInt32LE(eocd + 12); let offset = bytes.readUInt32LE(eocd + 16), total = 0;
  if (count > 4000 || offset + centralSize !== eocd) throw new Error('Invalid ZIP directory');
  const seen = new Set<string>(), folded = new Set<string>();
  await fs.mkdir(destination, { recursive: false, mode: 0o700 });
  for (let index = 0; index < count; index++) {
    if (offset + 46 > eocd || bytes.readUInt32LE(offset) !== 0x02014b50) throw new Error('Invalid ZIP entry');
    const flags = bytes.readUInt16LE(offset + 8), method = bytes.readUInt16LE(offset + 10), compressed = bytes.readUInt32LE(offset + 20), size = bytes.readUInt32LE(offset + 24);
    const nameLength = bytes.readUInt16LE(offset + 28), extra = bytes.readUInt16LE(offset + 30), comment = bytes.readUInt16LE(offset + 32), mode = bytes.readUInt32LE(offset + 38) >>> 16, local = bytes.readUInt32LE(offset + 42);
    if (offset + 46 + nameLength + extra + comment > eocd) throw new Error('Invalid ZIP entry length');
    const path = bytes.subarray(offset + 46, offset + 46 + nameLength).toString('utf8'); offset += 46 + nameLength + extra + comment;
    packagePath(path.endsWith('/') ? path.slice(0, -1) : path);
    if (flags & 1 || ![0, 8].includes(method) || (mode & 0xf000) === 0xa000 || ((mode & 0xf000) && ![0x8000, 0x4000].includes(mode & 0xf000))) throw new Error('Unsupported package entry');
    if (path.endsWith('/')) continue;
    const expected = files.get(path), identity = path.toLocaleLowerCase('en-US');
    if (!expected || seen.has(path) || folded.has(identity) || size !== expected.sizeBytes || size > MAX_EXPANDED || total + size > MAX_EXPANDED) throw new Error('Package inventory mismatch');
    seen.add(path); folded.add(identity); total += size;
    if (local + 30 > bytes.length || bytes.readUInt32LE(local) !== 0x04034b50) throw new Error('Invalid ZIP local entry');
    const localNameLength = bytes.readUInt16LE(local + 26), start = local + 30 + localNameLength + bytes.readUInt16LE(local + 28);
    if (bytes.subarray(local + 30, local + 30 + localNameLength).toString('utf8') !== path || start + compressed > eocd) throw new Error('ZIP entry identity mismatch');
    const packed = bytes.subarray(start, start + compressed), content = method === 0 ? packed : inflateRawSync(packed, { maxOutputLength: Math.max(1, size) });
    if (content.length !== size || sha256(content) !== expected.sha256) throw new Error('Package file digest mismatch');
    const file = join(destination, path); await fs.mkdir(dirname(file), { recursive: true, mode: 0o700 });
    await fs.writeFile(file, content, { flag: 'wx', mode: expected.executable ? 0o700 : 0o600 });
  }
  if (offset !== eocd || seen.size !== files.size) throw new Error('Incomplete package');
}
function crc32(bytes: Buffer): number { let value = 0xffffffff; for (const byte of bytes) { value ^= byte; for (let n = 0; n < 8; n++) value = (value >>> 1) ^ ((value & 1) ? 0xedb88320 : 0); } return (value ^ 0xffffffff) >>> 0; }
/** Stored ZIP output: files have already been allowlisted and verified by core export. */
export async function packSource(root: string, files: { path: string; executable?: boolean }[]): Promise<Buffer> {
  const contents: Buffer[] = [], central: Buffer[] = []; let offset = 0;
  for (const file of files) {
    packagePath(file.path); const name = Buffer.from(file.path), bytes = await fs.readFile(join(root, file.path));
    if (offset + bytes.length > MAX_ARCHIVE || files.length > 2000) throw new Error('Source package exceeds upload limit');
    const header = Buffer.alloc(30); header.writeUInt32LE(0x04034b50); header.writeUInt16LE(20, 4); header.writeUInt16LE(0x800, 6); header.writeUInt32LE(crc32(bytes), 14); header.writeUInt32LE(bytes.length, 18); header.writeUInt32LE(bytes.length, 22); header.writeUInt16LE(name.length, 26);
    const entry = Buffer.alloc(46); entry.writeUInt32LE(0x02014b50); entry.writeUInt16LE(0x314, 4); entry.writeUInt16LE(20, 6); entry.writeUInt16LE(0x800, 8); entry.writeUInt32LE(crc32(bytes), 16); entry.writeUInt32LE(bytes.length, 20); entry.writeUInt32LE(bytes.length, 24); entry.writeUInt16LE(name.length, 28); entry.writeUInt32LE(((file.executable ? 0o100700 : 0o100600) << 16) >>> 0, 38); entry.writeUInt32LE(offset, 42);
    contents.push(header, name, bytes); central.push(entry, name); offset += header.length + name.length + bytes.length;
  }
  const directory = Buffer.concat(central), end = Buffer.alloc(22); end.writeUInt32LE(0x06054b50); end.writeUInt16LE(files.length, 8); end.writeUInt16LE(files.length, 10); end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16);
  const archive = Buffer.concat([...contents, directory, end]); if (archive.length > MAX_ARCHIVE) throw new Error('Source package exceeds upload limit'); return archive;
}
