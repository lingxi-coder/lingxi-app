export const SUPPORTED_IMAGE_MEDIA_TYPES = [
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
] as const;

export type SupportedImageMediaType = (typeof SUPPORTED_IMAGE_MEDIA_TYPES)[number];

export const MAX_IMAGE_ATTACHMENTS = 4;
export const MAX_IMAGE_BYTES = 20 * 1024 * 1024;

export function isSupportedImageMediaType(value: unknown): value is SupportedImageMediaType {
  return typeof value === 'string'
    && (SUPPORTED_IMAGE_MEDIA_TYPES as readonly string[]).includes(value);
}

function startsWith(bytes: Uint8Array, signature: readonly number[]): boolean {
  return signature.every((value, index) => bytes[index] === value);
}

/** Detect the canonical media type from the file bytes, not its extension. */
export function detectImageMediaType(bytes: Uint8Array): SupportedImageMediaType | null {
  if (startsWith(bytes, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return 'image/png';
  if (startsWith(bytes, [0xff, 0xd8, 0xff])) return 'image/jpeg';
  if (startsWith(bytes, [0x47, 0x49, 0x46, 0x38, 0x37, 0x61])
    || startsWith(bytes, [0x47, 0x49, 0x46, 0x38, 0x39, 0x61])) return 'image/gif';
  if (startsWith(bytes, [0x52, 0x49, 0x46, 0x46])
    && startsWith(bytes.subarray(8), [0x57, 0x45, 0x42, 0x50])) return 'image/webp';
  return null;
}

/** Encode binary data without using a spread over the whole image. */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}
