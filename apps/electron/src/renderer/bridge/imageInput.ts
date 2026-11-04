import type { ImageRefDto } from '@lingxi/bridge-client';

import {
  bytesToBase64,
  detectImageMediaType,
  MAX_IMAGE_BYTES,
  type SupportedImageMediaType,
} from '../../shared/imageInput';

export interface ImageAttachment extends ImageRefDto {
  id: string;
  name: string;
  previewUrl: string;
  media_type: SupportedImageMediaType;
}

function imageFileName(file: File): string {
  return file.name.trim() || 'pasted-image';
}

export async function imageFileToAttachment(file: File): Promise<ImageAttachment> {
  if (file.size === 0 || file.size > MAX_IMAGE_BYTES) {
    throw new Error('Images must be between 1 byte and 20 MiB.');
  }
  const bytes = new Uint8Array(await file.arrayBuffer());
  const mediaType = detectImageMediaType(bytes);
  if (!mediaType) throw new Error('Only PNG, JPEG, GIF, and WebP images are supported.');
  const base64 = bytesToBase64(bytes);
  // One base64 copy is what submit needs. A real File previews from a blob URL
  // so the composer does not keep a second copy of that string. A stand-in
  // that is not a Blob, or a URL implementation that rejects it, falls back
  // to the data URL. Callers already revokeObjectURL on removal.
  let previewUrl = `data:${mediaType};base64,${base64}`;
  if (typeof URL !== 'undefined' && typeof URL.createObjectURL === 'function' && file instanceof Blob) {
    try {
      previewUrl = URL.createObjectURL(file);
    } catch {
      // Keep the data URL. Submit still has the base64 copy.
    }
  }
  return {
    id: `${Date.now()}-${Math.random().toString(36).slice(2)}`,
    name: imageFileName(file),
    media_type: mediaType,
    base64,
    previewUrl,
  };
}
