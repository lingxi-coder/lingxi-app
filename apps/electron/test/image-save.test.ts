import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseImageSave, registerImageSave } from '../src/main/imageSave';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import { CH_IMAGE_SAVE } from '../src/shared/imageSave';

test('image save IPC rejects unregistered and child frames and unregisters on disposal', async () => {
  const root = mkdtempSync(join(tmpdir(), 'lingxi-icon-ipc-'));
  const handlers = new Map<string, (...args: any[]) => any>();
  const ipc = { handle: (name: string, handler: (...args: any[]) => any) => handlers.set(name, handler), removeHandler: (name: string) => handlers.delete(name) };
  const host = new HostController(new SettingsStore(root), { registerIpc() {}, registerWindow() {} } as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, removeListener() {}, send() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  const invoke = handlers.get(CH_IMAGE_SAVE)!;
  try {
    await assert.rejects(invoke({ sender, senderFrame: { url: frame.url } }, 'com.apple.TextEdit'), /unauthorized/);
    await assert.rejects(invoke({ sender: { mainFrame: frame }, senderFrame: frame }, 'com.apple.TextEdit'), /unauthorized/);
    await assert.rejects(invoke({ sender, senderFrame: frame }, { dataUrl: 'https://example.com/image.png' }), /Invalid image/);
  } finally { host.dispose(); rmSync(root, { recursive: true, force: true }); }
  assert.ok(!handlers.has(CH_IMAGE_SAVE));
});

const png = Buffer.from('89504e470d0a1a0a', 'hex');
const request = { dataUrl: `data:image/png;base64,${png.toString('base64')}`, suggestedName: '../../photo.exe' };
test('image save validates payloads and forces the detected extension', () => {
  assert.equal(parseImageSave(request).name, 'photo.png');
  for (const dataUrl of ['file:///tmp/image', 'data:image/jpeg;base64,' + png.toString('base64'), request.dataUrl + '!']) {
    assert.throws(() => parseImageSave({ ...request, dataUrl }), /Invalid image/);
  }
});
test('image save cancellation and revoked sender cannot write a file', async () => {
  let handler: any;
  let trusted = true;
  registerImageSave({ handle: (_name: string, callback: any) => { handler = callback; } } as any,
    () => { if (!trusted) throw new Error('unauthorized'); }, async () => ({ canceled: true }));
  assert.equal(await handler({}, request), false);
  registerImageSave({ handle: (_name: string, callback: any) => { handler = callback; } } as any,
    () => { if (!trusted) throw new Error('unauthorized'); }, async () => { trusted = false; return { canceled: false, filePath: '/must-not-be-written' }; });
  await assert.rejects(handler({}, request), /unauthorized/);
});
