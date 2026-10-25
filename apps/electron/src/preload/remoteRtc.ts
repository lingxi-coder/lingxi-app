import { ipcRenderer } from 'electron';
import { CH_REMOTE_RTC_PORT, REMOTE_RTC_PORT_MESSAGE } from '../shared/remoteRtcPort.js';

// The hidden WebRTC page gets the one MessagePort main hands out, and nothing
// else: no API is exposed to it, and the port arrives once.
ipcRenderer.once(CH_REMOTE_RTC_PORT, (event) => {
  window.postMessage(REMOTE_RTC_PORT_MESSAGE, '*', [...event.ports]);
});
