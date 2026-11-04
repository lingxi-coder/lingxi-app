const { contextBridge, ipcRenderer } = require('electron');
contextBridge.exposeInMainWorld('lingxi', { saveImage: request => ipcRenderer.invoke('lingxi:image:save', request) });
