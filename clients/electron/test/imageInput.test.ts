import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  bytesToBase64,
  detectImageMediaType,
} from '../src/shared/imageInput';
import { imageFileToAttachment } from '../src/renderer/bridge/imageInput';

const PNG = Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00]);

test('image input detects supported formats from magic bytes', () => {
  assert.equal(detectImageMediaType(PNG), 'image/png');
  assert.equal(detectImageMediaType(Uint8Array.from([0xff, 0xd8, 0xff, 0x00])), 'image/jpeg');
  assert.equal(detectImageMediaType(new TextEncoder().encode('GIF89a')), 'image/gif');
  assert.equal(detectImageMediaType(Uint8Array.from([
    0x52, 0x49, 0x46, 0x46, 0x00, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50,
  ])), 'image/webp');
  assert.equal(detectImageMediaType(new Uint8Array([0x25, 0x50, 0x44, 0x46])), null);
});

test('image input encodes binary data as base64 without truncation', () => {
  const bytes = Uint8Array.from([0, 1, 2, 250, 251, 252]);
  assert.equal(bytesToBase64(bytes), Buffer.from(bytes).toString('base64'));
});

test('image input converts a File into an attachment with a preview URL', async () => {
  const file = new File([PNG], 'capture.png', { type: 'image/png' });
  const attachment = await imageFileToAttachment(file);
  try {
    assert.equal(attachment.name, 'capture.png');
    assert.equal(attachment.media_type, 'image/png');
    assert.equal(attachment.base64, Buffer.from(PNG).toString('base64'));
  assert.match(attachment.previewUrl, /^data:image\/png;base64,/);
  } finally {
    URL.revokeObjectURL(attachment.previewUrl);
  }
});

test('image input rejects unsupported and oversized files', async () => {
  await assert.rejects(
    imageFileToAttachment(new File([new TextEncoder().encode('not an image')], 'notes.txt', { type: 'text/plain' })),
    /Only PNG, JPEG, GIF, and WebP/,
  );
  await assert.rejects(
    imageFileToAttachment(new File([new Uint8Array(20 * 1024 * 1024 + 1)], 'large.png', { type: 'image/png' })),
    /between 1 byte and 20 MiB/,
  );
});
