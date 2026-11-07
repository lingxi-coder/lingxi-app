import type { ClientCommand, ClientEvent } from '@lingxi/bridge-client';
import type { DecisionInput, DecisionResult } from '../shared/decision.js';

/** Host-only facade. Configuration and credentials never cross into Agent inputs. */
export interface DesktopDecisionService {
  isConfigured(): Promise<boolean>;
  onConfigurationChanged(listener: () => void): () => void;
  evaluateTool(input: DecisionInput, signal: AbortSignal, timeoutMs: number): Promise<DecisionResult>;
}

/** One authenticated runtime connection; ordinary tool execution owns its lifetime. */
export class DecisionToolBridge {
  private readonly pending = new Map<string, AbortController>();
  private readonly seen = new Set<string>();
  private closed = false;
  private revision = 0;
  private advertised: boolean | undefined;
  private readonly unsubscribe?: () => void;

  constructor(private readonly service: DesktopDecisionService | undefined, private readonly send: (command: ClientCommand) => void) {
    this.unsubscribe = service?.onConfigurationChanged(() => { void this.refreshConfiguration(); });
  }

  async refreshConfiguration(): Promise<void> {
    const revision = ++this.revision;
    const configured = await this.service?.isConfigured().catch(() => false) ?? false;
    if (this.closed || revision !== this.revision || configured === this.advertised) return;
    try {
      this.send({ type: 'decision_configuration_changed', configured });
      this.advertised = configured;
    } catch { /* A retired connection cannot advertise a capability. */ }
  }

  handle(event: Extract<ClientEvent, { type: 'decision_request' | 'decision_cancel' }>): void {
    if (this.closed) return;
    const id = event.request_id;
    if (!/^[a-f0-9-]{36}$/.test(id)) return;
    if (event.type === 'decision_cancel') {
      this.pending.get(id)?.abort();
      return;
    }
    if (this.seen.has(id)) return;
    // Bound duplicate fencing for the lifetime of this connection. Never replay an old call.
    if (this.seen.size >= 4096) { this.respond(id, { error: 'Decision connection request limit reached' }); return; }
    this.seen.add(id);
    if (this.pending.size >= 8) { this.respond(id, { error: 'Decision service is busy' }); return; }
    if (!this.service) { this.respond(id, { error: 'Decision is unavailable on this host' }); return; }
    const service = this.service;
    const abort = new AbortController();
    this.pending.set(id, abort);
    void (async () => {
      try {
        if (Buffer.byteLength(event.input_json) > 16 * 1024 * 1024) throw new Error('Decision input exceeds 16 MiB');
        if (!Number.isSafeInteger(event.timeout_ms) || event.timeout_ms < 1 || event.timeout_ms > 30_000) throw new Error('Invalid Decision timeout');
        const input: unknown = JSON.parse(event.input_json);
        if (!input || typeof input !== 'object' || Array.isArray(input) || Object.keys(input).some(key => !['state', 'questions', 'images'].includes(key))) throw new Error('Invalid Decision input');
        const result = await service.evaluateTool(input as DecisionInput, abort.signal, event.timeout_ms);
        if (!abort.signal.aborted) this.respond(id, { result });
      } catch (error) {
        this.respond(id, { error: abort.signal.aborted ? 'Decision operation cancelled' : error instanceof Error ? error.message : 'Decision operation failed' });
      } finally {
        this.pending.delete(id);
      }
    })();
  }

  private respond(requestId: string, value: { result?: DecisionResult; error?: string }): void {
    if (this.closed) return;
    try { this.send({ type: 'decision_response', request_id: requestId, result_json: JSON.stringify(value) }); }
    catch { /* Disconnect owns cancellation; late responses are never reattached. */ }
  }

  dispose(): void {
    if (this.closed) return;
    this.closed = true;
    this.revision++;
    this.unsubscribe?.();
    for (const abort of this.pending.values()) abort.abort();
    this.pending.clear();
    this.seen.clear();
  }
}
