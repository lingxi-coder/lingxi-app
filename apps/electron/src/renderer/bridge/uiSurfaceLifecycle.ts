import type { AllowedClientCommand } from '../../shared/clientCommands.js';

type UiSurfaceCommandHost = {
  command(sessionId: string, command: AllowedClientCommand): Promise<void>;
};

/** Serialize mount/unmount commands so switching sessions detaches the old
 * surface before attaching it to the new session, including async IPC delay. */
export class UiSurfaceLifecycleQueue {
  private tail: Promise<void> = Promise.resolve();

  attach(host: UiSurfaceCommandHost, sessionId: string, clientId: string): Promise<void> {
    return this.enqueue(host, sessionId, { type: 'ui_attach', surface: 'desktop', client_id: clientId });
  }

  detach(host: UiSurfaceCommandHost, sessionId: string, clientId: string): Promise<void> {
    return this.enqueue(host, sessionId, { type: 'ui_detach', client_id: clientId });
  }

  private enqueue(
    host: UiSurfaceCommandHost,
    sessionId: string,
    command: AllowedClientCommand,
  ): Promise<void> {
    const operation = this.tail.catch(() => undefined).then(() => host.command(sessionId, command));
    this.tail = operation.catch(() => undefined);
    return operation;
  }
}

/** One stable identifier per renderer mount; reconnects reuse the same value. */
export function createUiSurfaceClientId(): string {
  return globalThis.crypto.randomUUID();
}
