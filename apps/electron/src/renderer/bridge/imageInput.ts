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
  return {
    id: `${Date.now()}-${Math.random().toString(36).slice(2)}`,
    name: imageFileName(file),
    media_type: mediaType,
    base64,
    previewUrl: `data:${mediaType};base64,${base64}`,
  };
}
