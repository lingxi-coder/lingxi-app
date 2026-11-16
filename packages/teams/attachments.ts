import { executionCommand, type ExecutionScope } from './execution.js';
import type { MessageApi } from './messages.js';

/**
 * Files in a Team message. A file is uploaded for one conversation in parts, the service checks the whole of it, and a message then
 * carries it. Its bytes are read back through the service every time, so they are only ever shown to someone who reads that conversation now.
 */
export const ATTACHMENT_PART_BYTES = 700 * 1024, ATTACHMENT_FILE_BYTES = 10 * 1024 * 1024, ATTACHMENT_MESSAGE_FILES = 5, ATTACHMENT_MESSAGE_BYTES = 20 * 1024 * 1024;
export const ATTACHMENT_TYPES = ['image/png', 'image/jpeg', 'application/pdf', 'text/plain'] as const;
export type AttachmentType = typeof ATTACHMENT_TYPES[number];
/** What a file picker is asked for. */
export const ATTACHMENT_ACCEPT = 'image/png,image/jpeg,application/pdf,text/plain,.png,.jpg,.jpeg,.pdf,.txt,.md,.csv,.log';
/** A file as a message carries it. `revoked`: its sender took it back and its bytes are gone. */
export interface MessageAttachment { attachmentId: string; name: string; mime: AttachmentType; byteLength: number; sha256: string; revoked?: boolean }
export interface UploadView { uploadId: string; state: 'uploading' | 'verified' | 'attached' | 'failed' | 'revoked'; parts: number; partBytes: number; expiresAt?: number; code?: string }
/** A file in an unsent message. `sent` is how many of its bytes the service has. */
export interface DraftFile { uploadId: string; name: string; mime: AttachmentType; byteLength: number; state: 'uploading' | 'ready' | 'failed'; sent: number; error?: string }
export type FileRefusal = 'ATTACHMENT_TYPE' | 'ATTACHMENT_EMPTY' | 'ATTACHMENT_TOO_LARGE' | 'ATTACHMENT_COUNT' | 'ATTACHMENT_TOTAL';

const TEXT_NAMES = /\.(txt|md|markdown|csv|log)$/iu;
/** The type a file is sent as, from what the system says about it and from its name; `undefined` for a kind of file Teams does not take. */
export function attachmentType(name: string, type: string): AttachmentType | undefined {
  const given = type.toLowerCase().split(';')[0]!.trim();
  if (given === 'image/png' || given === 'image/jpeg' || given === 'application/pdf') return given;
  if (given === 'image/jpg') return 'image/jpeg';
  if (given === 'text/plain' || given === 'text/markdown' || given === 'text/csv') return 'text/plain';
  if (given) return undefined;
  if (/\.png$/iu.test(name)) return 'image/png';
  if (/\.jpe?g$/iu.test(name)) return 'image/jpeg';
  if (/\.pdf$/iu.test(name)) return 'application/pdf';
  return TEXT_NAMES.test(name) ? 'text/plain' : undefined;
}
/** A file's own name as the service takes it: no path, no control characters, at most 120 characters with its ending kept. */
export function attachmentName(name: string): string {
  const base = (name.split(/[/\\]/u).pop() ?? '').replace(/[\u0000-\u001f\u007f]/gu, '').trim();
  if (!base || base === '.' || base === '..') return 'file';
  if (base.length <= 120) return base;
  // The limit is in UTF-16 units, as the service counts; a character is never cut in half.
  const dot = base.lastIndexOf('.'), ending = dot > 0 && base.length - dot <= 10 ? base.slice(dot) : '';
  let stem = '';
  for (const point of base.slice(0, base.length - ending.length)) { if (stem.length + point.length > 120 - ending.length) break; stem += point; }
  return `${stem.trimEnd()}${ending}`;
}
export const isImage = (file: Pick<MessageAttachment, 'mime'>): boolean => file.mime === 'image/png' || file.mime === 'image/jpeg';
/** 1.4 MB, 320 KB, 12 B. */
export function fileSize(bytes: number, locale = 'en-US'): string {
  const [value, unit] = bytes >= 1024 * 1024 ? [bytes / (1024 * 1024), 'MB'] : bytes >= 1024 ? [bytes / 1024, 'KB'] : [bytes, 'B'];
  return `${value.toLocaleString(locale, { maximumFractionDigits: unit === 'MB' ? 1 : 0 })} ${unit}`;
}

/**
 * Which of the chosen files go into a draft that already holds `draft`, in the order they were chosen, and why each of the others
 * does not: not a kind Teams takes, empty, over 10 MiB, a sixth file, or more than 20 MiB together.
 */
export function admitFiles<F extends { name: string; type: string; size: number }>(draft: readonly Pick<DraftFile, 'byteLength'>[], files: readonly F[]): {
  accepted: { file: F; name: string; mime: AttachmentType }[]; refused: { name: string; code: FileRefusal }[];
} {
  const accepted: { file: F; name: string; mime: AttachmentType }[] = [], refused: { name: string; code: FileRefusal }[] = [];
  let count = draft.length, total = draft.reduce((sum, file) => sum + file.byteLength, 0);
  for (const file of files) {
    const mime = attachmentType(file.name, file.type), name = attachmentName(file.name);
    const code: FileRefusal | undefined = !mime ? 'ATTACHMENT_TYPE' : file.size === 0 ? 'ATTACHMENT_EMPTY' : file.size > ATTACHMENT_FILE_BYTES ? 'ATTACHMENT_TOO_LARGE'
      : count >= ATTACHMENT_MESSAGE_FILES ? 'ATTACHMENT_COUNT' : total + file.size > ATTACHMENT_MESSAGE_BYTES ? 'ATTACHMENT_TOTAL' : undefined;
    if (code) { refused.push({ name, code }); continue; }
    accepted.push({ file, name, mime: mime! }); count += 1; total += file.size;
  }
  return { accepted, refused };
}

const hex = (bytes: ArrayBuffer) => [...new Uint8Array(bytes)].map(value => value.toString(16).padStart(2, '0')).join('');
/** A view of `bytes` whose buffer is exactly those bytes, as WebCrypto wants it. */
const exact = (bytes: Uint8Array): ArrayBuffer => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
export const sha256Hex = async (bytes: Uint8Array): Promise<string> => hex(await crypto.subtle.digest('SHA-256', exact(bytes)));
function base64(bytes: Uint8Array): string {
  let text = '';
  for (let at = 0; at < bytes.length; at += 0x8000) text += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
  return btoa(text);
}
function fromBase64(text: string): Uint8Array {
  const raw = atob(text), bytes = new Uint8Array(raw.length);
  for (let index = 0; index < raw.length; index += 1) bytes[index] = raw.charCodeAt(index);
  return bytes;
}
/** Answers that will not change by asking again. Anything else (no connection, a busy service) is tried again. */
const FINAL = /^(UPLOAD_|ATTACHMENT_|CONVERSATION_REQUIRED$|MEMBERSHIP_REQUIRED$|STALE_MEMBERSHIP$|INVALID_|SIGN_IN_REQUIRED$|REQUEST_TOO_LARGE$|COMMAND_ID_CONFLICT$)/u;
const codeOf = (error: unknown) => error instanceof Error ? error.message : '';
async function insist<T>(signal: AbortSignal | undefined, wait: (ms: number) => Promise<void>, step: () => Promise<T>): Promise<T> {
  for (let attempt = 0; ; attempt += 1) {
    signal?.throwIfAborted();
    try { return await step(); }
    catch (error) { if (attempt >= 3 || FINAL.test(codeOf(error)) || signal?.aborted) throw error; await wait(500 * 2 ** attempt); }
  }
}
type Api = Pick<MessageApi, 'messages'>;
export interface UploadOptions { signal?: AbortSignal; onProgress?(sent: number): void; /** Test seam: the pause before a step is tried again. */ wait?(ms: number): Promise<void> }

/**
 * Upload one file for one conversation and have it checked. Each step has its own stable command id, so a step repeated after a lost
 * answer is the same step, and the whole upload can be started again with the same `uploadId`. Throws the service's code when the file
 * is refused (`UPLOAD_TYPE_MISMATCH`, `UPLOAD_DIGEST_MISMATCH`, `UPLOAD_QUOTA_EXCEEDED`, …).
 */
export async function uploadAttachment(api: Api, scope: ExecutionScope, conversationId: string, uploadId: string,
  file: { name: string; mime: AttachmentType; bytes: Uint8Array }, options: UploadOptions = {}): Promise<void> {
  const { signal, onProgress } = options, wait = options.wait ?? (ms => new Promise<void>(resolve => setTimeout(resolve, ms)));
  const sha256 = await sha256Hex(file.bytes);
  const created = await insist(signal, wait, () => api.messages<UploadView>(executionCommand(scope, 'createUpload',
    { conversationId, uploadId, name: file.name, mime: file.mime, byteLength: file.bytes.length, sha256 }, `${uploadId}_c`)));
  if (created.state === 'verified') { onProgress?.(file.bytes.length); return; }
  if (created.state !== 'uploading') throw new Error(created.code ?? 'UPLOAD_FAILED');
  for (let index = 0; index < created.parts; index += 1) {
    const data = base64(file.bytes.subarray(index * created.partBytes, (index + 1) * created.partBytes));
    await insist(signal, wait, () => api.messages(executionCommand(scope, 'putUploadPart', { uploadId, index, data }, `${uploadId}_p${index}`)));
    onProgress?.(Math.min(file.bytes.length, (index + 1) * created.partBytes));
  }
  const checked = await insist(signal, wait, () => api.messages<UploadView>(executionCommand(scope, 'finalizeUpload', { uploadId }, `${uploadId}_f`)));
  if (checked.state !== 'verified') throw new Error(checked.code ?? 'UPLOAD_FAILED');
}

/** A file of a message, whole and checked against what the message says it is. Asked of the service every time; nothing is kept. */
export async function downloadAttachment(api: Api, scope: ExecutionScope, file: MessageAttachment, signal?: AbortSignal): Promise<Uint8Array> {
  const read = async (index: number) => { signal?.throwIfAborted();
    return api.messages<{ parts: number; data: string }>(executionCommand(scope, 'readAttachmentPart', { attachmentId: file.attachmentId, index })); };
  const first = await read(0), parts = [fromBase64(first.data)];
  for (let index = 1; index < first.parts; index += 1) parts.push(fromBase64((await read(index)).data));
  const bytes = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) { bytes.set(part, at); at += part.length; }
  if (bytes.length !== file.byteLength || await sha256Hex(bytes) !== file.sha256) throw new Error('ATTACHMENT_DAMAGED');
  return bytes;
}
