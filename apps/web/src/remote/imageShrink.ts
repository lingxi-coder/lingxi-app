import type { ImageRefDto } from '@lingxi/bridge-client';
import { bytesToBase64, isSupportedImageMediaType } from './reuse';

/** What the desktop accepts per image. */
export const MAX_UPLOAD_BYTES = 2 * 1024 * 1024;
/** The longest edge of an uploaded image; larger pictures are scaled down. */
export const MAX_UPLOAD_EDGE = 1568;

/** Dimensions that fit `width` x `height` inside a square of `maxEdge`, never enlarging. */
export function fitWithin(width: number, height: number, maxEdge: number): { width: number; height: number } {
  const scale = Math.min(1, maxEdge / Math.max(width, height));
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

/**
 * The next thing to try when an encoded image is still too big: lower the
 * JPEG quality first, then shrink the picture and start again at good quality.
 */
export function nextAttempt(quality: number, scale: number): { quality: number; scale: number } {
  return quality > 0.55 ? { quality: Math.round((quality - 0.15) * 100) / 100, scale } : { quality: 0.8, scale: scale * 0.75 };
}

/** Why a picked file could not be attached. */
export class ImageRejected extends Error {
  constructor(readonly reason: 'type' | 'decode' | 'size') {
    super(reason);
    this.name = 'ImageRejected';
  }
}

/**
 * A picked file as an `ImageRefDto` of at most 2 MiB: small supported files
 * whose longest edge is already within bounds go through untouched; anything
 * else is drawn on a canvas, scaled, and re-encoded as JPEG until it fits.
 */
export async function shrinkImage(file: Blob): Promise<ImageRefDto> {
  if (!isSupportedImageMediaType(file.type)) throw new ImageRejected('type');
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new ImageRejected('decode');
  }
  try {
    const target = fitWithin(bitmap.width, bitmap.height, MAX_UPLOAD_EDGE);
    if (file.size <= MAX_UPLOAD_BYTES && target.width === bitmap.width && target.height === bitmap.height) {
      return { media_type: file.type, base64: bytesToBase64(new Uint8Array(await file.arrayBuffer())) };
    }
    let attempt = { quality: 0.9, scale: 1 };
    for (let tries = 0; tries < 12; tries++) {
      const blob = await encodeJpeg(bitmap, Math.max(1, Math.round(target.width * attempt.scale)), Math.max(1, Math.round(target.height * attempt.scale)), attempt.quality);
      if (blob.size <= MAX_UPLOAD_BYTES) return { media_type: 'image/jpeg', base64: bytesToBase64(new Uint8Array(await blob.arrayBuffer())) };
      attempt = nextAttempt(attempt.quality, attempt.scale);
    }
    throw new ImageRejected('size');
  } finally {
    bitmap.close();
  }
}

function encodeJpeg(bitmap: ImageBitmap, width: number, height: number, quality: number): Promise<Blob> {
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext('2d');
  if (context === null) return Promise.reject(new ImageRejected('decode'));
  context.fillStyle = '#ffffff'; // JPEG has no transparency
  context.fillRect(0, 0, width, height);
  context.drawImage(bitmap, 0, 0, width, height);
  return new Promise<Blob>((resolve, reject) => {
    canvas.toBlob((blob) => (blob === null ? reject(new ImageRejected('decode')) : resolve(blob)), 'image/jpeg', quality);
  });
}
