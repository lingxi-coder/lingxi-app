import type { TerminalEvent, TerminalSnapshot } from '../shared/terminal.js';

interface Pending {
  sequence: number;
  chunks: { data: string; sequence: number }[];
  size: number;
  inFlight?: number;
  timer?: ReturnType<typeof setTimeout>;
}

/** One acknowledged frame at a time, with lossless upstream flow control. */
export class TerminalDelivery {
  private readonly terminals = new Map<string, Pending>();
  constructor(
    private readonly send: (event: TerminalEvent) => void,
    private readonly setPaused: (paused: boolean) => void = () => {},
  ) {}

  watch(snapshot: TerminalSnapshot): void {
    // Listing another tab must not discard this tab's undelivered data.
    if (this.terminals.has(snapshot.id)) return;
    this.terminals.set(snapshot.id, { sequence: snapshot.sequence, chunks: [], size: 0 });
  }

  has(id: string): boolean { return this.terminals.has(id); }

  push(event: TerminalEvent): void {
    const pending = this.terminals.get(event.terminalId);
    if (!pending) return;
    if (event.kind !== 'output' && event.kind !== 'reset') {
      this.send(event);
      if (event.kind === 'closed') {
        if (pending.timer) clearTimeout(pending.timer);
        this.terminals.delete(event.terminalId);
        this.updatePressure();
      }
      return;
    }
    if (event.sequence <= pending.sequence) return;
    pending.sequence = event.sequence;
    // A `reset` clears the emulator's screen, so it must not be coalesced into
    // the `output` frame the scheduler emits — the buffered chunk carries only
    // `data` and `sequence`, so the kind would be lost and the renderer would
    // append the reset's bytes to the stale scrollback instead of clearing it.
    // Everything buffered before a reset is about to be wiped anyway, so drop it
    // and send the reset on its own.
    if (event.kind === 'reset') {
      if (pending.timer) { clearTimeout(pending.timer); pending.timer = undefined; }
      pending.chunks = [];
      pending.size = 0;
      pending.inFlight = event.sequence;
      this.send(event);
      this.updatePressure();
      return;
    }
    pending.chunks.push({ data: event.data, sequence: event.sequence });
    pending.size += event.data.length;
    this.updatePressure();
    this.schedule(event.terminalId, pending);
  }

  acknowledge(id: string, sequence: number): void {
    const pending = this.terminals.get(id);
    if (!pending || sequence > pending.sequence || (pending.inFlight !== undefined && sequence < pending.inFlight)) return;
    pending.inFlight = undefined;
    // A fresh snapshot can cover several queued frames; keep only newer bytes.
    pending.chunks = pending.chunks.filter(chunk => chunk.sequence > sequence);
    pending.size = pending.chunks.reduce((size, chunk) => size + chunk.data.length, 0);
    this.updatePressure();
    this.schedule(id, pending);
  }

  private updatePressure(): void {
    this.setPaused([...this.terminals.values()].some(pending => pending.size >= 65_536));
  }

  private schedule(id: string, pending: Pending): void {
    if (pending.timer || pending.inFlight !== undefined || !pending.chunks.length) return;
    pending.timer = setTimeout(() => {
      pending.timer = undefined;
      if (!pending.chunks.length) return;
      const sequence = pending.chunks[pending.chunks.length - 1]!.sequence;
      const data = pending.chunks.map(chunk => chunk.data).join('');
      pending.inFlight = sequence;
      pending.chunks = [];
      pending.size = 0;
      this.send({ kind: 'output', terminalId: id, data, sequence });
      this.updatePressure();
    }, 16);
  }

  dispose(): void {
    for (const pending of this.terminals.values()) if (pending.timer) clearTimeout(pending.timer);
    this.terminals.clear();
    this.setPaused(false);
  }
}
