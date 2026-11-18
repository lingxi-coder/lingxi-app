import { ipcMain, type BrowserWindow } from 'electron';
import type { InboxService } from './service.js';
import { bindInboxIpc } from './bindIpc.js';

export const installInboxIpc = (service: InboxService, windows: () => readonly BrowserWindow[], rendererUrl: string) =>
  bindInboxIpc(ipcMain, service, windows, rendererUrl);
