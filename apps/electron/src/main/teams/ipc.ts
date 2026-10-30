import { ipcMain, type BrowserWindow } from 'electron';
import type { TeamsService } from './service.js';

export function installTeamsIpc(service: TeamsService, windows: () => readonly BrowserWindow[]): () => void {
  const channel = 'lingxi:teams';
  ipcMain.handle(channel, async (event, input: unknown) => {
    if (!windows().some(target => event.sender === target.webContents && event.senderFrame === target.webContents.mainFrame)) throw new Error('Invalid sender');
    if (!input || typeof input !== 'object') throw new Error('Invalid Team command');
    const command = input as Record<string, unknown>;
    if (typeof command.uid !== 'string' || !command.uid || command.uid.length > 128) throw new Error('Invalid account');
    try {
      let value: unknown;
      switch (command.type) {
        case 'list':
          value = await service.list(command.uid);
          break;
        case 'localBots':
          value = await service.localBots(command.uid);
          break;
        case 'bindLocalBot':
          if (!command.input || typeof command.input !== 'object') throw new Error('INVALID_INPUT');
          value = await service.bindLocalBot(command.uid, command.input as Parameters<TeamsService['bindLocalBot']>[1]);
          break;
        case 'detail':
          if (typeof command.teamId !== 'string') throw new Error('INVALID_INPUT');
          value = await service.detail(command.teamId, command.uid);
          break;
        case 'messages':
          if (!command.command || typeof command.command !== 'object') throw new Error('INVALID_INPUT');
          value = await service.messages(command.command as Parameters<TeamsService['messages']>[0], command.uid);
          break;
        case 'execute':
          if (!command.command || typeof command.command !== 'object') throw new Error('INVALID_INPUT');
          value = await service.execute(command.command as Parameters<TeamsService['execute']>[0], command.uid);
          break;
        default:
          throw new Error('INVALID_INPUT');
      }
      return { value };
    } catch (error) {
      // Electron's invoke error serialization discards custom error fields.
      const code = error instanceof Error && /^[A-Z_]{3,60}$/u.test(error.message) ? error.message : 'SERVICE_UNAVAILABLE';
      return { error: code };
    }
  });
  const stop = service.onChanged(event => { for (const target of windows()) if (!target.isDestroyed()) target.webContents.send('lingxi:teams:changed', event); });
  const stopMessages = service.onMessagesChanged(() => { for (const target of windows()) if (!target.isDestroyed()) target.webContents.send('lingxi:teams:messages'); });
  return () => { stopMessages(); stop(); ipcMain.removeHandler(channel); service.close(); };
}
