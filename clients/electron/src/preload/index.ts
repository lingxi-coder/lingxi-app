import { contextBridge } from 'electron';

// The UI shell has no backend wiring yet (engine/bridge integration is a later
// milestone). We expose a minimal, typed surface so the renderer can detect it
// is running inside Electron.
const api = {
  platform: process.platform,
  isElectron: true,
};

if (process.contextIsolated) {
  try {
    contextBridge.exposeInMainWorld('lingxi', api);
  } catch (error) {
    console.error(error);
  }
} else {
  // @ts-expect-error fallback when context isolation is disabled
  window.lingxi = api;
}

export type LingxiApi = typeof api;
