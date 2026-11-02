import type { IpcMain, IpcMainInvokeEvent, SaveDialogOptions, SaveDialogReturnValue } from 'electron';
import { writeFile } from 'node:fs/promises';
import { CH_IMAGE_SAVE } from '../shared/imageSave.js';
import { detectImageMediaType, MAX_IMAGE_BYTES } from '../shared/imageInput.js';

export function parseImageSave(value: unknown) {
  if (!value || typeof value !== 'object') throw new Error('Invalid image');
  const { dataUrl, suggestedName } = value as Record<string, unknown>;
  if (typeof dataUrl !== 'string' || dataUrl.length > Math.ceil(MAX_IMAGE_BYTES / 3) * 4 + 64) throw new Error('Invalid image');
  const match = /^data:(image\/(?:png|jpeg|gif|webp));base64,([A-Za-z0-9+/]+={0,2})$/.exec(dataUrl);
  if (!match) throw new Error('Invalid image');
  const bytes = Buffer.from(match[2]!, 'base64');
  if (bytes.length > MAX_IMAGE_BYTES || bytes.toString('base64') !== match[2] || detectImageMediaType(bytes) !== match[1]) throw new Error('Invalid image');
  const extension = match[1] === 'image/jpeg' ? 'jpg' : match[1]!.slice(6);
  const name = (typeof suggestedName === 'string' ? suggestedName : 'image').split(/[\\/]/).pop()!
    .replace(/[\x00-\x1f\x7f<>:"|?*]/g, '').replace(/^\.+/, '').replace(/\.[^.]*$/, '').trim().slice(0, 160) || 'image';
  return { bytes, extension, name: `${name}.${extension}` };
}

export function registerImageSave(ipc: Pick<IpcMain, 'handle'>, assertSender: (event: IpcMainInvokeEvent) => void,
  showSaveDialog: (options: SaveDialogOptions) => Promise<SaveDialogReturnValue>) {
  ipc.handle(CH_IMAGE_SAVE, async (event, value: unknown) => {
    assertSender(event);
    const image = parseImageSave(value);
    const result = await showSaveDialog({ defaultPath: image.name, filters: [{ name: image.extension.toUpperCase(), extensions: [image.extension] }] });
    assertSender(event);
    if (result.canceled || !result.filePath) return false;
    await writeFile(result.filePath, image.bytes, { mode: 0o600 });
    return true;
  });
}
