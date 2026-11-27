import { ipcMain, type BrowserWindow } from 'electron';
import { FriendApiError } from '../../../../../packages/teams/friends';
import type { FriendsService } from './friends.js';

/** `lingxi:friends`: the Friends actions, profile reads and the three watches, each bound to the account the renderer names. */
export function installFriendsIpc(service: FriendsService, windows: () => readonly BrowserWindow[]): () => void {
  const channel = 'lingxi:friends';
  const senders = new Map<number, () => void>();
  const searches = new Map<string, AbortController>();
  const trackSender = (sender: BrowserWindow['webContents']) => {
    const senderId = sender.id;
    if (senders.has(senderId)) return;
    const prefix = `${senderId}:`;
    const release = () => {
      service.releaseWatches(prefix);
      for (const [key, controller] of searches) if (key.startsWith(prefix)) {
        searches.delete(key);
        controller.abort();
      }
    };
    const navigate = (_event: unknown, _url: string, inPlace: boolean, main: boolean) => { if (main && !inPlace) release(); };
    const destroyed = () => { release(); senders.get(senderId)?.(); senders.delete(senderId); };
    sender.on('destroyed', destroyed); sender.on('did-start-navigation', navigate);
    senders.set(senderId, () => { sender.off('destroyed', destroyed); sender.off('did-start-navigation', navigate); });
  };
  ipcMain.handle(channel, async (event, input: unknown) => {
    if (!windows().some(target => event.sender === target.webContents && event.senderFrame === target.webContents.mainFrame)) throw new Error('Invalid sender');
    if (!input || typeof input !== 'object') throw new Error('Invalid Friends command');
    const command = input as Record<string, unknown>;
    if (typeof command.uid !== 'string' || !command.uid || command.uid.length > 128) throw new Error('Invalid account');
    try {
      let value: unknown;
      switch (command.type) {
        case 'watch':
        case 'unwatch': {
          if (typeof command.leaseId !== 'string' || !/^[A-Za-z0-9_-]{8,64}$/u.test(command.leaseId)) throw new Error('INVALID_INPUT');
          const prefix = `${event.sender.id}:`;
          const lease = `${prefix}${command.leaseId}:`;
          if (command.type === 'unwatch') { service.releaseWatches(lease); break; }
          if (command.collection !== 'friends' && command.collection !== 'blocks' && command.collection !== 'teamOffers') throw new Error('INVALID_INPUT');
          trackSender(event.sender);
          await service.watch(lease, command.uid, command.collection);
          break;
        }
        case 'profile':
          value = await service.profile(command.uid);
          break;
        case 'execute':
          if (!command.command || typeof command.command !== 'object') throw new Error('INVALID_INPUT');
          value = await service.execute(command.uid, command.command as Parameters<FriendsService['execute']>[1]);
          break;
        case 'searchHandles':
        case 'cancelSearch': {
          if (typeof command.requestId !== 'string' || !/^[A-Za-z0-9_-]{8,64}$/u.test(command.requestId)) throw new Error('INVALID_INPUT');
          const key = `${event.sender.id}:${command.uid}:${command.requestId}`;
          if (command.type === 'cancelSearch') { searches.get(key)?.abort(); break; }
          if (typeof command.query !== 'string' || searches.has(key)) throw new Error('INVALID_INPUT');
          trackSender(event.sender);
          const controller = new AbortController();
          searches.set(key, controller);
          try { value = await service.execute(command.uid, { action: 'searchHandles', query: command.query }, controller.signal); }
          finally { if (searches.get(key) === controller) searches.delete(key); }
          break;
        }
        case 'readProfile':
          if (typeof command.friendUid !== 'string') throw new Error('INVALID_INPUT');
          value = await service.readProfile(command.uid, command.friendUid);
          break;
        default:
          throw new Error('INVALID_INPUT');
      }
      return { value };
    } catch (error) {
      // Electron's invoke error serialization discards custom error fields.
      const code = error instanceof Error && /^[A-Z_]{3,60}$/u.test(error.message) ? error.message : 'SERVICE_UNAVAILABLE';
      return { error: code, ...(error instanceof FriendApiError && error.retryAfterMs !== undefined ? { retryAfterMs: error.retryAfterMs } : {}) };
    }
  });
  const send = (lease: string, name: string, payload: unknown) => {
    const [senderId, leaseId] = lease.split(':');
    for (const target of windows()) if (!target.isDestroyed() && String(target.webContents.id) === senderId) target.webContents.send(name, { ...(payload as object), leaseId });
  };
  const stopRows = service.onRows(({ leaseId, rows }) => send(leaseId, 'lingxi:friends:rows', { rows }));
  const stopFailed = service.onFailed(({ leaseId, code }) => send(leaseId, 'lingxi:friends:failed', { code }));
  return () => { for (const search of searches.values()) search.abort(); searches.clear(); for (const release of senders.values()) release(); senders.clear(); stopRows(); stopFailed(); ipcMain.removeHandler(channel); service.close(); };
}
