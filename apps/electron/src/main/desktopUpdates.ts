import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { ipcMain, net, shell, type IpcMainInvokeEvent, type WebContents } from 'electron';
import { CH_DESKTOP_UPDATES, CH_DESKTOP_UPDATES_CHANGED } from '../shared/desktopUpdates.js';
import { DesktopUpdateService, isHttpsUpdateUrl } from './desktopUpdateService.js';

const MAX_MANIFEST_BYTES = 256 * 1_024;

export async function fetchUpdateManifest(url: string): Promise<unknown> {
  if (!isHttpsUpdateUrl(url)) throw new Error('Desktop update manifest requires HTTPS');
  const response = await net.fetch(url, {
    credentials: 'omit', redirect: 'error', cache: 'no-store', signal: AbortSignal.timeout(15_000),
  });
  if (!response.ok || !response.body) throw new Error(`Desktop update check failed (${response.status})`);
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > MAX_MANIFEST_BYTES) throw new Error('Desktop update manifest is too large');
      chunks.push(value);
    }
  } finally { await reader.cancel(); }
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

export function installDesktopUpdates(options: {
  currentVersion: string;
  manifestUrl: string;
  preferencePath: string;
  onError(error: unknown): void;
}) {
  const windows = new Map<WebContents, string>();
  const service = new DesktopUpdateService({
    currentVersion: options.currentVersion,
    platform: process.platform,
    arch: process.arch,
    fetchManifest: () => fetchUpdateManifest(options.manifestUrl),
    openExternal: url => shell.openExternal(url),
    readSnooze: () => JSON.parse(readFileSync(options.preferencePath, 'utf8')),
    saveSnooze: value => {
      mkdirSync(dirname(options.preferencePath), { recursive: true });
      const temporary = `${options.preferencePath}.tmp`;
      writeFileSync(temporary, JSON.stringify(value) + '\n', { encoding: 'utf8', mode: 0o600 });
      renameSync(temporary, options.preferencePath);
    },
    onChanged: state => {
      for (const [contents, url] of windows) {
        if (!contents.isDestroyed() && contents.mainFrame.url === url) contents.send(CH_DESKTOP_UPDATES_CHANGED, state);
      }
    },
    onError: options.onError,
  });
  const assertSender = (event: IpcMainInvokeEvent) => {
    const url = windows.get(event.sender);
    if (!url || !event.senderFrame || event.senderFrame !== event.sender.mainFrame || event.senderFrame.url !== url) {
      throw new Error('Unauthorized Desktop update request');
    }
  };
  ipcMain.handle(CH_DESKTOP_UPDATES, async (event, action: unknown) => {
    assertSender(event);
    if (action === 'get') return service.getState();
    if (action === 'download') return service.download();
    if (action === 'later') return service.remindLater();
    throw new Error('Unknown Desktop update action');
  });
  return {
    registerWindow(contents: WebContents, url: string): void {
      windows.set(contents, url);
      contents.once('destroyed', () => windows.delete(contents));
    },
    start: () => service.start(),
    check: () => service.check(),
    dispose(): void {
      service.dispose(); windows.clear(); ipcMain.removeHandler(CH_DESKTOP_UPDATES);
    },
  };
}
