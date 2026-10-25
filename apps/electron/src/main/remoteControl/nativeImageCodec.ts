/**
 * Electron's `nativeImage` as the {@link ImageCodec} that fits an image into
 * what `image.get` may return. One of the files in this directory that import
 * Electron's runtime values (see `electronBindings.ts`).
 */

import { nativeImage } from 'electron';

import type { ImageCodec } from './remoteCommands.js';

export const nativeImageCodec: ImageCodec = {
  size(bytes) {
    const image = nativeImage.createFromBuffer(Buffer.from(bytes));
    if (image.isEmpty()) return undefined;
    const { width, height } = image.getSize();
    return width > 0 && height > 0 ? { width, height } : undefined;
  },
  scale(bytes, width, height, encode) {
    const image = nativeImage.createFromBuffer(Buffer.from(bytes));
    if (image.isEmpty()) return undefined;
    const scaled = image.resize({ width, height, quality: 'best' });
    const out = encode.type === 'png' ? scaled.toPNG() : scaled.toJPEG(encode.quality);
    return out.length > 0 ? new Uint8Array(out) : undefined;
  },
};
