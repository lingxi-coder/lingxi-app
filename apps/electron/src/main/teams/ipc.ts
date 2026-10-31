import { ipcMain, type BrowserWindow } from 'electron';
import type { TeamsService } from './service.js';

export function installTeamsIpc(service: TeamsService, windows: () => readonly BrowserWindow[]): () => void {
  const channel = 'lingxi:teams';
  const senders = new Map<number, () => void>();
  ipcMain.handle(channel, async (event, input: unknown) => {
    if (!windows().some(target => event.sender === target.webContents && event.senderFrame === target.webContents.mainFrame)) throw new Error('Invalid sender');
    if (!input || typeof input !== 'object') throw new Error('Invalid Team command');
    const command = input as Record<string, unknown>;
    if (typeof command.uid !== 'string' || !command.uid || command.uid.length > 128) throw new Error('Invalid account');
    try {
      let value: unknown;
      switch (command.type) {
        case 'watchMessages':
        case 'unwatchMessages': {
          if (typeof command.leaseId !== 'string' || !/^[A-Za-z0-9_-]{8,64}$/u.test(command.leaseId)) throw new Error('INVALID_INPUT');
          const prefix = `${event.sender.id}:`;
          const lease = `${prefix}${command.leaseId}:`;
          if (command.type === 'unwatchMessages') { service.releaseMessageWatches(lease); break; }
          if (!command.scope || typeof command.scope !== 'object' || typeof command.conversationId !== 'string') throw new Error('INVALID_INPUT');
          if (!senders.has(event.sender.id)) {
            const sender = event.sender, senderId = sender.id;
            const release = () => service.releaseMessageWatches(prefix);
            const navigate = (_event: unknown, _url: string, inPlace: boolean, main: boolean) => { if (main && !inPlace) release(); };
            const destroyed = () => { release(); senders.get(senderId)?.(); senders.delete(senderId); };
            sender.on('destroyed', destroyed); sender.on('did-start-navigation', navigate);
            senders.set(sender.id, () => { sender.off('destroyed', destroyed); sender.off('did-start-navigation', navigate); });
          }
          await service.watchMessages(lease, command.uid, command.scope as Parameters<TeamsService['watchMessages']>[2], command.conversationId);
          break;
        }
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
  const stopSnapshots = service.onMessageSnapshot(event => {
    const [senderId, leaseId] = event.leaseId.split(':');
    for (const target of windows()) if (!target.isDestroyed() && String(target.webContents.id) === senderId) target.webContents.send('lingxi:teams:snapshot', { leaseId, snapshot: event.snapshot });
  });
  const stopMessages = service.onMessagesChanged(signal => { for (const target of windows()) if (!target.isDestroyed()) target.webContents.send('lingxi:teams:messages', signal); });
  return () => { for (const release of senders.values()) release(); senders.clear(); stopSnapshots(); stopMessages(); stop(); ipcMain.removeHandler(channel); service.close(); };
}
