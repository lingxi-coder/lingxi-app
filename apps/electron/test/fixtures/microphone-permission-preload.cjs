/**
 * A real `contextBridge` preload for the microphone-permission fixture,
 * mirroring the one production line it stands in for
 * (`src/preload/index.ts`'s `microphoneAccess`) — same channel name, same
 * `ipcRenderer.invoke`, same sandboxed/context-isolated window.
 */
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('lingxi', {
  microphoneAccess: () => ipcRenderer.invoke('lingxi:microphone-access:get'),
});
