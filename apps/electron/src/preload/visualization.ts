/**
 * Preload for inline visualization guests. Runs in the shell's main frame only
 * (`nodeIntegrationInSubFrames` is off), so the sandboxed content frame never
 * sees this bridge. It carries opaque JSON strings between the shell and the
 * embedding renderer and nothing else.
 */

import { contextBridge, ipcRenderer } from 'electron';

const CHANNEL = 'lingxi-visualization';
const MAX_MESSAGE_CHARS = 64 * 1024;

let deliver: ((raw: string) => void) | null = null;

ipcRenderer.on(CHANNEL, (_event, raw: unknown) => {
  if (typeof raw === 'string' && raw.length <= MAX_MESSAGE_CHARS) deliver?.(raw);
});

contextBridge.exposeInMainWorld('lingxiVisualization', {
  postMessage(raw: unknown): void {
    if (typeof raw === 'string' && raw.length <= MAX_MESSAGE_CHARS) ipcRenderer.sendToHost(CHANNEL, raw);
  },
  onHostMessage(callback: unknown): void {
    if (deliver === null && typeof callback === 'function') deliver = callback as (raw: string) => void;
  },
});
