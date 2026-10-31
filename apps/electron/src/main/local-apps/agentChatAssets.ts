import { constants as fsConstants, promises as fs } from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import { join } from 'node:path';

const PRIVATE_ASSET = '__lingxiLocalAppAsset';
const HASH = /^[a-f0-9]{64}$/;
const DEFAULTS = {
  maxFilesPerMessage: 8,
  maxFileBytes: 25 * 1024 * 1024,
  maxNewMessageBytes: 50 * 1024 * 1024,
  maxChatAssetBytes: 256 * 1024 * 1024,
  maxHistoryResponseBytes: 72 * 1024 * 1024,
} as const;

export class LocalAppChatAssetError extends Error {
  constructor(readonly code: string, message: string) { super(message); this.name = 'LocalAppChatAssetError'; }
}

export interface LocalAppChatAssetLimits {
  maxFilesPerMessage?: number;
  maxFileBytes?: number;
  maxNewMessageBytes?: number;
  maxChatAssetBytes?: number;
  maxHistoryResponseBytes?: number;
}

export interface StoredAgentMessage {
  /** Standard public message JSON with inline bytes replaced only in this private on-disk form. */
  message: unknown;
  assetHashes: string[];
  newMessageBytes: number;
}

function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}

function cloneJson(value: unknown): unknown {
  try { return JSON.parse(JSON.stringify(value)); }
  catch { throw new LocalAppChatAssetError('invalid_message', 'The Agent message is not JSON.'); }
}

export function decodeInlineAgentData(value: unknown, maxBytes = DEFAULTS.maxFileBytes): Buffer {
  if (typeof value !== 'string' || value.length === 0 || value.length % 4 !== 0) {
    throw new LocalAppChatAssetError('invalid_attachment', 'An inline Agent attachment is invalid.');
  }
  const maxEncodedLength = 4 * Math.ceil(maxBytes / 3);
  if (value.length > maxEncodedLength) throw new LocalAppChatAssetError('attachment_too_large', 'An Agent attachment exceeds the file size limit.');
  const padding = value.endsWith('==') ? 2 : value.endsWith('=') ? 1 : 0;
  const contentLength = value.length - padding;
  if ((padding === 1 && contentLength % 4 !== 3) || (padding === 2 && contentLength % 4 !== 2)
    || (padding === 0 && contentLength % 4 !== 0)) {
    throw new LocalAppChatAssetError('invalid_attachment', 'An inline Agent attachment is invalid.');
  }
  for (let index = 0; index < contentLength; index += 1) {
    const code = value.charCodeAt(index);
    const alpha = code >= 65 && code <= 90 || code >= 97 && code <= 122;
    const digit = code >= 48 && code <= 57;
    if (!alpha && !digit && code !== 43 && code !== 47) throw new LocalAppChatAssetError('invalid_attachment', 'An inline Agent attachment is invalid.');
  }
  for (let index = contentLength; index < value.length; index += 1) {
    if (value.charCodeAt(index) !== 61) throw new LocalAppChatAssetError('invalid_attachment', 'An inline Agent attachment is invalid.');
  }
  const bytes = Buffer.from(value, 'base64');
  if (bytes.length > maxBytes) throw new LocalAppChatAssetError('attachment_too_large', 'An Agent attachment exceeds the file size limit.');
  if (bytes.length === 0 || bytes.toString('base64') !== value) throw new LocalAppChatAssetError('invalid_attachment', 'An inline Agent attachment is invalid.');
  return bytes;
}

function sha256(value: Buffer): string { return createHash('sha256').update(value).digest('hex'); }

function noFollowFlag(): number { return fsConstants.O_NOFOLLOW ?? 0; }
async function syncAssetDirectory(path: string): Promise<void> {
  if (process.platform === 'win32') return;
  const directory = await fs.open(path, 'r');
  try { await directory.sync(); } finally { await directory.close(); }
}


async function assertPrivateDirectory(path: string, mode: number): Promise<void> {
  await fs.mkdir(path, { recursive: true, mode });
  const stat = await fs.lstat(path);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new LocalAppChatAssetError('asset_storage_invalid', 'The Agent chat attachment storage is unsafe.');
  if (process.platform !== 'win32') await fs.chmod(path, mode);
}

async function readVerifiedAsset(path: string, digest: string, expectedBytes: number, maxFileBytes: number): Promise<Buffer> {
  if (!HASH.test(digest) || !Number.isSafeInteger(expectedBytes) || expectedBytes < 1 || expectedBytes > maxFileBytes) {
    throw new LocalAppChatAssetError('asset_storage_invalid', 'The Agent chat attachment record is invalid.');
  }
  let handle;
  try {
    const stat = await fs.lstat(path);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size !== expectedBytes) throw new Error('unsafe asset');
    handle = await fs.open(path, fsConstants.O_RDONLY | noFollowFlag());
    const openedStat = await handle.stat();
    if (!openedStat.isFile() || openedStat.size !== expectedBytes) throw new Error('asset size changed');
    const bounded = Buffer.allocUnsafe(expectedBytes + 1);
    const { bytesRead } = await handle.read(bounded, 0, bounded.length, 0);
    if (bytesRead !== expectedBytes) throw new Error('asset size changed while reading');
    const bytes = bounded.subarray(0, expectedBytes);
    if (sha256(bytes) !== digest) throw new Error('asset digest mismatch');
    return bytes;
  } catch {
    throw new LocalAppChatAssetError('asset_storage_invalid', 'An Agent chat attachment failed its integrity check.');
  } finally { await handle?.close().catch(() => undefined); }
}

/**
 * Stores standard AG-UI inline image/document bytes as private, content-addressed
 * files. The message's public shape is preserved when it is rehydrated.
 */
export class LocalAppChatAssetJournal {
  private readonly limits: Required<LocalAppChatAssetLimits>;
  private readonly assetsDirectory: string;
  constructor(private readonly chatDirectory: string, limits: LocalAppChatAssetLimits = {}) {
    this.limits = { ...DEFAULTS, ...limits };
    this.assetsDirectory = join(chatDirectory, 'assets');
  }

  private async ensureDirectories(): Promise<void> {
    await assertPrivateDirectory(this.chatDirectory, 0o700);
    await assertPrivateDirectory(this.assetsDirectory, 0o700);
  }

  private async inventory(): Promise<{ bytes: number; hashes: Set<string> }> {
    await this.ensureDirectories();
    let bytes = 0;
    const hashes = new Set<string>();
    for (const name of await fs.readdir(this.assetsDirectory)) {
      if (name.startsWith('.') && name.endsWith('.tmp')) continue;
      const match = /^([a-f0-9]{64})\.bin$/.exec(name);
      if (!match) throw new LocalAppChatAssetError('asset_storage_invalid', 'The Agent chat attachment storage contains an invalid entry.');
      const stat = await fs.lstat(join(this.assetsDirectory, name));
      if (!stat.isFile() || stat.isSymbolicLink() || stat.size < 1 || stat.size > this.limits.maxFileBytes) {
        throw new LocalAppChatAssetError('asset_storage_invalid', 'The Agent chat attachment storage contains an unsafe file.');
      }
      hashes.add(match[1]!); bytes += stat.size;
      if (bytes > this.limits.maxChatAssetBytes) throw new LocalAppChatAssetError('chat_asset_limit', 'This Agent chat has reached its attachment storage limit.');
    }
    return { bytes, hashes };
  }

  private async writeAsset(digest: string, bytes: Buffer, alreadyStored: boolean): Promise<void> {
    const path = join(this.assetsDirectory, `${digest}.bin`);
    if (alreadyStored) {
      try { await readVerifiedAsset(path, digest, bytes.length, this.limits.maxFileBytes); await syncAssetDirectory(this.assetsDirectory); }
      catch { throw new LocalAppChatAssetError('asset_storage_failed', 'The Agent chat attachment could not be stored safely.'); }
      return;
    }
    const temporary = join(this.assetsDirectory, `.${randomUUID()}.tmp`);
    let handle;
    try {
      handle = await fs.open(temporary, fsConstants.O_WRONLY | fsConstants.O_CREAT | fsConstants.O_EXCL | noFollowFlag(), 0o600);
      await handle.writeFile(bytes);
      await handle.sync();
      await handle.close(); handle = undefined;
      try { await fs.link(temporary, path); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
        await readVerifiedAsset(path, digest, bytes.length, this.limits.maxFileBytes);
      }
      if (process.platform !== 'win32') await fs.chmod(path, 0o600);
      await syncAssetDirectory(this.assetsDirectory);
    } catch {
      throw new LocalAppChatAssetError('asset_storage_failed', 'The Agent chat attachment could not be stored safely.');
    } finally {
      await handle?.close().catch(() => undefined);
      await fs.rm(temporary, { force: true }).catch(() => undefined);
    }
  }

  /** Canonical form for transcript-prefix comparison without writing bytes. */
  canonicalizeMessage(message: unknown): unknown {
    const clone = cloneJson(message);
    const row = object(clone);
    if (!row || Object.hasOwn(row, PRIVATE_ASSET)) throw new LocalAppChatAssetError('invalid_message', 'The Agent message is invalid.');
    const content = row.content;
    if (!Array.isArray(content)) return clone;
    const canonicalContent = content.map(partValue => {
      const part = object(partValue);
      if (!part || (part.type !== 'image' && part.type !== 'document')) return partValue;
      const source = object(part.source);
      if (!source || source.type !== 'data') throw new LocalAppChatAssetError('unsupported_attachment_source', 'Use inline image or document data in Agent messages.');
      if (Object.hasOwn(source, PRIVATE_ASSET)) {
        const ref = object(source[PRIVATE_ASSET]);
        if (!ref || typeof ref.sha256 !== 'string' || !HASH.test(ref.sha256) || !Number.isSafeInteger(ref.byteLength)
          || (ref.byteLength as number) < 1 || (ref.byteLength as number) > this.limits.maxFileBytes || Object.hasOwn(source, 'value')) {
          throw new LocalAppChatAssetError('asset_storage_invalid', 'The Agent chat history contains an invalid attachment record.');
        }
        return { ...part, source: { ...source } };
      }
      const bytes = decodeInlineAgentData(source.value, this.limits.maxFileBytes);
      const canonicalSource = { ...source };
      delete canonicalSource.value;
      canonicalSource[PRIVATE_ASSET] = { sha256: sha256(bytes), byteLength: bytes.length };
      return { ...part, source: canonicalSource };
    });
    return { ...row, content: canonicalContent };
  }

  async storeMessage(message: unknown): Promise<StoredAgentMessage> {
    await this.ensureDirectories();
    const clone = cloneJson(message);
    const row = object(clone);
    if (!row || Object.hasOwn(row, PRIVATE_ASSET)) throw new LocalAppChatAssetError('invalid_message', 'The Agent message is invalid.');
    const content = row.content;
    if (!Array.isArray(content)) return { message: clone, assetHashes: [], newMessageBytes: 0 };

    const { bytes: existingBytes, hashes: existingHashes } = await this.inventory();
    const captured: Array<{ source: Record<string, unknown>; digest: string; bytes: Buffer }> = [];
    let inputBytes = 0;
    for (const partValue of content) {
      const part = object(partValue);
      if (!part || (part.type !== 'image' && part.type !== 'document')) continue;
      const source = object(part.source);
      if (!source || source.type !== 'data' || Object.hasOwn(source, PRIVATE_ASSET)) {
        throw new LocalAppChatAssetError('unsupported_attachment_source', 'Use inline image or document data in Agent messages.');
      }
      const bytes = decodeInlineAgentData(source.value, this.limits.maxFileBytes);
      inputBytes += bytes.length;
      captured.push({ source, bytes, digest: sha256(bytes) });
    }
    if (captured.length > this.limits.maxFilesPerMessage || inputBytes > this.limits.maxNewMessageBytes) {
      throw new LocalAppChatAssetError('attachment_limit', 'The Agent message exceeds its attachment count or size limit.');
    }
    const additions = new Map<string, Buffer>();
    for (const asset of captured) if (!existingHashes.has(asset.digest)) additions.set(asset.digest, asset.bytes);
    const additionalBytes = [...additions.values()].reduce((sum, bytes) => sum + bytes.length, 0);
    if (existingBytes + additionalBytes > this.limits.maxChatAssetBytes) {
      throw new LocalAppChatAssetError('chat_asset_limit', 'This Agent chat has reached its attachment storage limit.');
    }

    const storedContent = content.map(partValue => {
      const part = object(partValue);
      if (!part || (part.type !== 'image' && part.type !== 'document')) return partValue;
      const source = object(part.source)!;
      const asset = captured.find(row => row.source === source);
      if (!asset) throw new LocalAppChatAssetError('invalid_attachment', 'An inline Agent attachment is invalid.');
      const storedSource = { ...source };
      delete storedSource.value;
      storedSource[PRIVATE_ASSET] = { sha256: asset.digest, byteLength: asset.bytes.length };
      return { ...part, source: storedSource };
    });

    const storedMessage = { ...row, content: storedContent };
    for (const [digest, bytes] of additions) await this.writeAsset(digest, bytes, false);
    for (const asset of captured) if (existingHashes.has(asset.digest)) await this.writeAsset(asset.digest, asset.bytes, true);
    return { message: storedMessage, assetHashes: [...new Set(captured.map(asset => asset.digest))], newMessageBytes: inputBytes };
  }

  async rehydrateMessage(storedMessage: unknown): Promise<unknown> {
    const row = object(cloneJson(storedMessage));
    if (!row) throw new LocalAppChatAssetError('asset_storage_invalid', 'The Agent chat history is invalid.');
    const content = row.content;
    if (!Array.isArray(content)) return row;
    const hydrated = [];
    for (const partValue of content) {
      const part = object(partValue);
      if (!part || (part.type !== 'image' && part.type !== 'document')) { hydrated.push(partValue); continue; }
      const source = object(part.source);
      const asset = object(source?.[PRIVATE_ASSET]);
      if (!source || !asset || typeof asset.sha256 !== 'string' || typeof asset.byteLength !== 'number' || Object.hasOwn(source, 'value')) {
        throw new LocalAppChatAssetError('asset_storage_invalid', 'The Agent chat history contains an invalid attachment record.');
      }
      const bytes = await readVerifiedAsset(join(this.assetsDirectory, `${asset.sha256}.bin`), asset.sha256, asset.byteLength, this.limits.maxFileBytes);
      const restoredSource: Record<string, unknown> = { ...source, value: bytes.toString('base64') };
      delete restoredSource[PRIVATE_ASSET];
      hydrated.push({ ...part, source: restoredSource });
    }
    const message = { ...row, content: hydrated };
    if (Buffer.byteLength(JSON.stringify(message)) > this.limits.maxHistoryResponseBytes) {
      throw new LocalAppChatAssetError('history_message_too_large', 'This Agent history message is too large to return in one page.');
    }
    return message;
  }

  async rehydrateMessages(messages: readonly unknown[]): Promise<{ messages: unknown[]; serializedBytes: number }> {
    const result: unknown[] = [];
    let serializedBytes = 2;
    for (const stored of messages) {
      const message = await this.rehydrateMessage(stored);
      const bytes = Buffer.byteLength(JSON.stringify(message)) + (result.length ? 1 : 0);
      if (serializedBytes + bytes > this.limits.maxHistoryResponseBytes) {
        throw new LocalAppChatAssetError('history_page_too_large', 'This Agent history page exceeds the response size limit. Use a smaller message page.');
      }
      result.push(message); serializedBytes += bytes;
    }
    return { messages: result, serializedBytes };
  }

  async pruneUnreferenced(messages: readonly unknown[]): Promise<void> {
    const used = new Set<string>();
    const visit = (value: unknown) => {
      const row = object(value);
      if (!row) { if (Array.isArray(value)) value.forEach(visit); return; }
      const asset = object(row[PRIVATE_ASSET]);
      if (asset && typeof asset.sha256 === 'string' && HASH.test(asset.sha256)) used.add(asset.sha256);
      for (const child of Object.values(row)) visit(child);
    };
    messages.forEach(visit);
    const { hashes } = await this.inventory();
    for (const digest of hashes) if (!used.has(digest)) await fs.rm(join(this.assetsDirectory, `${digest}.bin`), { force: true });
  }
}
