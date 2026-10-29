import { ipcMain } from 'electron';
import type { BrowserWindow } from 'electron';
import type { TeamsService } from './service';

export function installTeamsIpc(service: TeamsService, windows: () => readonly BrowserWindow[]): () => void {
  const channel = 'lingxi:teams';
  ipcMain.handle(channel, async (event, input: unknown) => {
    if (!windows().some(target => event.sender === target.webContents && event.senderFrame === target.webContents.mainFrame)) throw new Error('Invalid sender');
    if (!input || typeof input !== 'object') throw new Error('Invalid Team command');
    const command = input as Record<string, unknown>;
    if (typeof command.uid !== 'string' || !command.uid || command.uid.length > 128) throw new Error('Invalid account');
    try {
      const value = command.type === 'list' ? await service.list(command.uid)
        : command.type === 'detail' && typeof command.teamId === 'string' ? await service.detail(command.teamId, command.uid)
        : command.type === 'execute' && command.command && typeof command.command === 'object' ? await service.execute(command.command as Parameters<TeamsService['execute']>[0], command.uid)
        : (() => { throw new Error('INVALID_INPUT'); })();
      return { value };
    } catch (error) {
      // Electron's invoke error serialization discards custom error fields.
      const code = error instanceof Error && /^[A-Z_]{3,60}$/u.test(error.message) ? error.message : 'SERVICE_UNAVAILABLE';
      return { error: code };
    }
  });
  const stop = service.onChanged(event => { for (const target of windows()) if (!target.isDestroyed()) target.webContents.send('lingxi:teams:changed', event); });
  return () => { stop(); ipcMain.removeHandler(channel); service.close(); };
}
