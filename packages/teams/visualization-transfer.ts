import type { VisualizationBundle, VisualizationScope } from './visualization.js';

export const visualizationPackageBytes = 50 * 1024 * 1024;
export const visualizationPartBytes = 2 * 1024 * 1024;
export const editableVisualizationChars = 128 * 1024;
const maxParts = Math.ceil(visualizationPackageBytes / visualizationPartBytes);
const hash = /^[a-f0-9]{64}$/u;
const assetName = /^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}$/u;
const base64 = /^[A-Za-z0-9+/]*={0,2}$/u;
const mimes = new Set(['image/png', 'image/jpeg', 'image/webp', 'text/css', 'text/javascript']);

type Request = <T>(body: Record<string, unknown>) => Promise<T>;

export type VisualizationPackage = {
  title: string;
  htmlBytes: number;
  assets: { name: string; mime: string; bytes: number }[];
  parts: string[];
};

export type VisualizationUpload = VisualizationScope & {
  status: 'uploading';
  message: string;
  version: string;
  missing: number[];
};

export type PreparedVisualization = {
  package: VisualizationPackage;
  readPart(index: number): Promise<ArrayBuffer>;
};

const sha256 = async (bytes: ArrayBuffer) =>
  Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)), byte => byte.toString(16).padStart(2, '0')).join('');

async function prepareVisualizationBytes(
  title: string, htmlBytes: number, assets: VisualizationPackage['assets'],
  read: (offset: number, end: number) => Promise<ArrayBuffer>,
): Promise<PreparedVisualization> {
  const size = assets.reduce((bytes, asset) => bytes + asset.bytes, htmlBytes);
  if (size > visualizationPackageBytes) throw new Error('VISUALIZATION_TOO_LARGE');
  if (htmlBytes < 1 || !title.trim() || title.trim().length > 80) throw new Error('INVALID_DATA');
  const readPart = (index: number) => {
    const start = index * visualizationPartBytes;
    return read(start, Math.min(size, start + visualizationPartBytes));
  };
  const parts: string[] = [];
  for (let start = 0; start < size; start += visualizationPartBytes) {
    const part = await readPart(parts.length);
    if (part.byteLength !== Math.min(visualizationPartBytes, size - start)) throw new Error('INVALID_DATA');
    parts.push(await sha256(part));
  }
  return { package: { title: title.trim(), htmlBytes, assets, parts }, readPart };
}

export async function prepareVisualizationPackage(bundle: VisualizationBundle): Promise<PreparedVisualization> {
  if (!bundle.html.trim()) throw new Error('INVALID_DATA');
  const html = new Blob([bundle.html]);
  const assets = Object.entries(bundle.assets ?? {});
  if (assets.length > 32 || assets.some(([name, asset]) =>
    !assetName.test(name) || !mimes.has(asset.mime) || !base64.test(asset.data))) throw new Error('INVALID_DATA');
  const source = new Blob([html, ...assets.map(([, asset]) => asset.data)]);
  return prepareVisualizationBytes(bundle.title, html.size,
    assets.map(([name, { mime, data }]) => ({ name, mime, bytes: data.length })),
    (start, end) => source.slice(start, end).arrayBuffer());
}

function toBase64(bytes: ArrayBuffer): string {
  const view = new Uint8Array(bytes) as Uint8Array & { toBase64?(): string };
  if (typeof view.toBase64 === 'function') return view.toBase64();
  let binary = '';
  for (let offset = 0; offset < view.length; offset += 16 * 1024) {
    binary += String.fromCharCode(...view.subarray(offset, offset + 16 * 1024));
  }
  return btoa(binary);
}

function fromBase64(data: string): Uint8Array<ArrayBuffer> {
  const bytes = Uint8Array as typeof Uint8Array & { fromBase64?(data: string): Uint8Array<ArrayBuffer> };
  if (typeof bytes.fromBase64 === 'function') return bytes.fromBase64(data);
  const binary = atob(data), result = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) result[index] = binary.charCodeAt(index);
  return result;
}

export async function uploadVisualizationParts(request: Request, transfer: PreparedVisualization, upload: VisualizationUpload) {
  if (!Array.isArray(upload.missing) || upload.missing.length > maxParts) throw new Error('INVALID_DATA');
  const { message, version, missing } = upload;
  for (const index of missing) {
    if (!Number.isSafeInteger(index) || !hash.test(transfer.package.parts[index] ?? '')) throw new Error('INVALID_DATA');
    const bytes = await transfer.readPart(index);
    if (await sha256(bytes) !== transfer.package.parts[index]) throw new Error('INVALID_DATA');
    await request({ action: 'putPart', message, version, index, data: toBase64(bytes) });
  }
}

function packageSize(value: VisualizationPackage): number {
  if (!value || typeof value.title !== 'string' || !value.title.trim() || value.title.length > 80
      || !Number.isSafeInteger(value.htmlBytes) || value.htmlBytes < 1 || value.htmlBytes > visualizationPackageBytes
      || !Array.isArray(value.assets) || value.assets.length > 32
      || !Array.isArray(value.parts) || value.parts.length > maxParts || !value.parts.every(digest => hash.test(digest))) {
    throw new Error('INVALID_DATA');
  }
  const bytes = value.assets.reduce((total, asset) => {
    if (!asset || !assetName.test(asset.name) || !mimes.has(asset.mime)
        || !Number.isSafeInteger(asset.bytes) || asset.bytes < 0 || asset.bytes > visualizationPackageBytes) throw new Error('INVALID_DATA');
    return total + asset.bytes;
  }, value.htmlBytes);
  if (bytes > visualizationPackageBytes || value.parts.length !== Math.ceil(bytes / visualizationPartBytes)
      || new Set(value.assets.map(asset => asset.name)).size !== value.assets.length) throw new Error('INVALID_DATA');
  return bytes;
}

export async function openVisualization(request: Request, signal?: AbortSignal): Promise<{ bundle: VisualizationBundle; state: unknown }> {
  const opened = await request<{ package: VisualizationPackage; state: unknown }>({ action: 'open' });
  const info = opened.package;
  const bytes = new Uint8Array(packageSize(info));
  for (let index = 0; index < info.parts.length; index++) {
    signal?.throwIfAborted();
    const { data } = await request<{ data: string }>({ action: 'readPart', index });
    const offset = index * visualizationPartBytes, size = Math.min(visualizationPartBytes, bytes.length - offset);
    if (typeof data !== 'string' || data.length !== Math.ceil(size / 3) * 4) throw new Error('INVALID_DATA');
    const chunk = fromBase64(data);
    if (chunk.length !== size || await sha256(chunk.buffer) !== info.parts[index]) throw new Error('INVALID_DATA');
    bytes.set(chunk, offset);
  }
  signal?.throwIfAborted();
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
  const html = decoder.decode(bytes.subarray(0, info.htmlBytes));
  if (!html.trim()) throw new Error('INVALID_DATA');
  let offset = info.htmlBytes;
  const assets = Object.fromEntries(info.assets.map(({ name, mime, bytes: size }) => {
    const data = decoder.decode(bytes.subarray(offset, offset + size));
    offset += size;
    if (!base64.test(data)) throw new Error('INVALID_DATA');
    return [name, { mime, data }];
  }));
  return { bundle: { title: info.title, html, assets }, state: opened.state };
}
