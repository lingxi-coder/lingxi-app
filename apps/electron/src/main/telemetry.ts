import { ROOT_CONTEXT, SpanStatusCode, type Span } from '@opentelemetry/api';
import { resourceFromAttributes } from '@opentelemetry/resources';
import { BasicTracerProvider, BatchSpanProcessor, type SpanExporter } from '@opentelemetry/sdk-trace-base';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-proto';
import type { BridgeTraceContext, ClientEvent } from '@lingxi/bridge-client';
import { telemetryPrivacyDisabled, type DesktopTelemetryConfig } from './telemetryPolicy.js';

/** A real host root span, exported by the SDK and continued by the Rust bridge. */
export class DesktopTelemetry {
  private readonly provider: BasicTracerProvider | undefined;
  private allowed: boolean;
  private readonly turns = new Map<string, { span: Span; turnId?: number; firstText: boolean; failed: boolean }>();
  private activeSpanId: string | undefined;

  constructor(config: DesktopTelemetryConfig, appVersion?: string, exporter?: SpanExporter) {
    this.allowed = config.enabled;
    if (!this.allowed) return;
    try {
      const output = exporter ?? new OTLPTraceExporter({
        url: `${config.endpoint}/v1/traces`, headers: config.headers,
        timeoutMillis: 1000, concurrencyLimit: 1,
      });
      // Revocation is checked at export time too, including shutdown's queue drain.
      const guarded: SpanExporter = {
        export: (spans, callback) => {
          if (!this.allowed || telemetryPrivacyDisabled(process.env)) { callback({ code: 0 }); return; }
          output.export(spans, callback);
        },
        shutdown: () => output.shutdown(),
      };
      this.provider = new BasicTracerProvider({
        resource: resourceFromAttributes({
          'service.name': 'lingxi-desktop',
          ...(appVersion && /^[0-9A-Za-z.+_-]{1,64}$/.test(appVersion) ? { 'service.version': appVersion } : {}),
          'os.type': process.platform,
        }),
        spanProcessors: [new BatchSpanProcessor(guarded, {
          maxQueueSize: 128, maxExportBatchSize: 32,
          scheduledDelayMillis: 1000, exportTimeoutMillis: 1500,
        })],
      });
    } catch { this.allowed = false; }
  }

  beginTurn(turnId?: number): BridgeTraceContext | undefined {
    if (!this.allowed || !this.provider || telemetryPrivacyDisabled(process.env) || this.turns.size >= 64) return undefined;
    const span = this.provider.getTracer('lingxi.desktop').startSpan('lingxi.desktop.request', {}, ROOT_CONTEXT);
    const context = span.spanContext();
    this.turns.set(context.spanId, { span, turnId, firstText: false, failed: false });
    // Submitted follow-ups may queue behind the foreground request. Their roots
    // remain open independently until their own turn becomes active and ends.
    this.activeSpanId ??= context.spanId;
    return { traceparent: `00-${context.traceId}-${context.spanId}-${context.traceFlags.toString(16).padStart(2, '0')}` };
  }

  onEvent(event: ClientEvent): void {
    if (event.type === 'turn_started') {
      if (event.turn_id === undefined) this.activeSpanId ??= this.turns.keys().next().value;
      else this.activeSpanId = [...this.turns].find(([, turn]) => turn.turnId === event.turn_id)?.[0]
        ?? [...this.turns].find(([, turn]) => turn.turnId === undefined)?.[0];
    }
    const turn = this.activeSpanId ? this.turns.get(this.activeSpanId) : undefined;
    if (event.type === 'session_ended') { this.endAll(); return; }
    if (!turn) return;
    if (event.type === 'error') turn.failed = true;
    if (event.type === 'text_delta' && !turn.firstText) {
      turn.firstText = true;
      turn.span.addEvent('lingxi.first_token');
    }
    if (event.type === 'turn_ended') {
      this.endTurn(event.outcome.type === 'cancelled' ? 'cancelled' : turn.failed ? 'error' : 'completed');
    }
  }

  endTurn(outcome: 'completed' | 'error' | 'cancelled' | 'interrupted', context?: BridgeTraceContext): void {
    const spanId = context?.traceparent.split('-')[2] ?? this.activeSpanId;
    const turn = spanId ? this.turns.get(spanId) : undefined;
    if (!spanId || !turn) return;
    this.turns.delete(spanId);
    if (this.activeSpanId === spanId) this.activeSpanId = undefined;
    turn.span.setAttribute('outcome', outcome);
    if (outcome === 'error') turn.span.setStatus({ code: SpanStatusCode.ERROR });
    turn.span.end();
  }

  /** A lost transport retires queued roots as well as the foreground owner. */
  interrupt(outcome: 'error' | 'interrupted' = 'interrupted'): void { this.endAll(outcome); }

  private endAll(outcome: 'error' | 'interrupted' = 'interrupted'): void {
    for (const [spanId, turn] of this.turns) {
      turn.span.setAttribute('outcome', outcome);
      if (outcome === 'error') turn.span.setStatus({ code: SpanStatusCode.ERROR });
      turn.span.end();
      this.turns.delete(spanId);
    }
    this.activeSpanId = undefined;
  }

  /** Drops queued spans before the SDK attempts a shutdown flush. */
  disable(): void {
    this.allowed = false;
    this.endAll();
  }

  async forceFlush(): Promise<void> { await this.provider?.forceFlush().catch(() => undefined); }

  async shutdown(): Promise<void> {
    this.endAll();
    await this.provider?.shutdown().catch(() => undefined);
  }
}
