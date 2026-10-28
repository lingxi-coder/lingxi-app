/** Desktop-only terminal transport. These events never enter conversation history. */
export const CH_TERMINAL_REQUEST = 'lingxi:terminal:request';
export const CH_TERMINAL_EVENT = 'lingxi:terminal:event';
export const TERMINAL_DRAFT_SESSION = '__draft__';

export interface TerminalScope { projectPath: string; sessionId: string }
export type TerminalUnknownReason = 'exit-observation-unavailable' | 'service-unavailable';
export interface TerminalSnapshot {
  id: string;
  scope: TerminalScope;
  title: string;
  status: 'running' | 'exited' | 'unknown';
  exitCode: number | null;
  unknownReason?: TerminalUnknownReason;
  output: string;
  sequence: number;
}
export type TerminalEvent =
  | { kind: 'output'; terminalId: string; data: string; sequence: number }
  | { kind: 'reset'; terminalId: string; data: string; sequence: number }
  | { kind: 'exit'; terminalId: string; exitCode: number }
  | { kind: 'unknown'; terminalId: string; reason: TerminalUnknownReason }
  | { kind: 'closed'; terminalId: string }
  | { kind: 'scope'; terminalId: string; scope: TerminalScope };
export interface TerminalApi {
  list(scope: TerminalScope): Promise<TerminalSnapshot[]>;
  create(scope: TerminalScope): Promise<TerminalSnapshot>;
  input(terminalId: string, data: string): Promise<void>;
  resize(terminalId: string, cols: number, rows: number): Promise<void>;
  close(terminalId: string): Promise<void>;
  acknowledge(terminalId: string, sequence: number): Promise<void>;
  onEvent(callback: (event: TerminalEvent) => void): () => void;
}
