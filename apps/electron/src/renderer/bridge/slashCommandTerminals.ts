import type { ClientEvent } from '@lingxi/bridge-client';

export interface SlashCommandTerminalResult {
  turnId: number;
  display: string;
  isError: boolean;
}

type Handler = (result: SlashCommandTerminalResult) => void;

/** Correlates slash outcomes to the request that supplied their turn ID. */
export class SlashCommandTerminalRegistry {
  private readonly handlers = new Map<string, Map<number, Handler>>();

  register(sessionId: string, turnId: number, handler: Handler): void {
    const sessionHandlers = this.handlers.get(sessionId) ?? new Map<number, Handler>();
    sessionHandlers.set(turnId, handler);
    this.handlers.set(sessionId, sessionHandlers);
  }

  observe(sessionId: string, event: ClientEvent): boolean {
    if ((event.type !== 'slash_command_result' && event.type !== 'turn_started') || event.turn_id === undefined) {
      return false;
    }
    const sessionHandlers = this.handlers.get(sessionId);
    const handler = sessionHandlers?.get(event.turn_id);
    if (!handler) return false;
    sessionHandlers!.delete(event.turn_id);
    if (sessionHandlers!.size === 0) this.handlers.delete(sessionId);
    handler(event.type === 'slash_command_result'
      ? { turnId: event.turn_id, display: event.display, isError: event.is_error === true }
      : { turnId: event.turn_id, display: '', isError: false });
    return true;
  }

  clearSession(sessionId: string): void {
    this.handlers.delete(sessionId);
  }

  clear(): void {
    this.handlers.clear();
  }
}
