import { ipcRenderer } from 'electron';

ipcRenderer.once('lingxi:teams:data-port', event => {
  if (event.ports.length === 1) window.postMessage('lingxi:teams:data-port', '*', event.ports);
});
