import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ReadableSpan, SpanExporter } from '@opentelemetry/sdk-trace-base';
import { BridgeClient } from '../../../packages/bridge-client/src/client.js';
import type { Frame } from '../../../packages/bridge-client/src/protocol.js';
import { DesktopTelemetry } from '../src/main/telemetry.js';
import { parseTelemetryEndpoint, protectBridgeTelemetryEnvironment, resolveDesktopTelemetry } from '../src/main/telemetryPolicy.js';
import { SettingsStore } from '../src/main/settings.js';

const enabled = resolveDesktopTelemetry({ LINGXI_ENABLE_TELEMETRY: 'true' });
function recorder(): { spans: ReadableSpan[]; exporter: SpanExporter } {
  const spans: ReadableSpan[] = [];
  return { spans, exporter: { export: (batch, done) => { spans.push(...batch); done({ code: 0 }); }, shutdown: async () => {} } };
}

test('device consent is default-off, durable, and keeps its endpoint private', () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-telemetry-settings-'));
  try {
    assert.equal(resolveDesktopTelemetry({}).enabled, false);
    writeFileSync(join(dir, 'settings.v1.json'), JSON.stringify({ version: 1, telemetryEnabled: false, telemetryEndpoint: 'https://metrics.example/otlp' }));
    const settings = new SettingsStore(dir);
    assert.equal(settings.getTelemetryConfig({ LINGXI_ENABLE_TELEMETRY: 'true' }).enabled, false);
    assert.equal('telemetryEndpoint' in settings.getPublic(), false);
    settings.update({ telemetryEnabled: true });
    assert.equal(new SettingsStore(dir).getTelemetryConfig({}).enabled, true);
    assert.equal(JSON.parse(readFileSync(settings.settingsPath, 'utf8')).telemetryEnabled, true);
    assert.equal(settings.getTelemetryConfig({ DO_NOT_TRACK: 'true' }).enabled, false);
    assert.throws(() => settings.update({ telemetryEnabled: 'true' } as never));
    assert.throws(() => settings.update({ telemetryEnabled: false, theme: 'dark' }));
    assert.equal(settings.getTelemetryConfig({}).enabled, true);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('only HTTPS or loopback HTTP endpoints are accepted and invalid endpoints disable reporting', () => {
  for (const invalid of ['http://external.example', 'file:///tmp/export', 'https://user:secret@example.com', 'https://example.com?a=secret', 'https://example.com/#secret']) {
    assert.equal(parseTelemetryEndpoint(invalid), undefined);
    assert.equal(resolveDesktopTelemetry({ LINGXI_ENABLE_TELEMETRY: 'true', OTEL_EXPORTER_OTLP_ENDPOINT: invalid }).enabled, false);
  }
  assert.equal(parseTelemetryEndpoint('http://[::1]:4318/'), 'http://[::1]:4318');
});

test('unreadable persisted consent cannot fall back to a host environment opt-in', () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-telemetry-unreadable-'));
  try {
    writeFileSync(join(dir, 'settings.v1.json'), '{broken consent');
    assert.equal(new SettingsStore(dir).getTelemetryConfig({ LINGXI_ENABLE_TELEMETRY: 'true' }).enabled, false);
    assert.equal(readFileSync(join(dir, 'settings.v1.json'), 'utf8'), '{broken consent');
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test('workspace environment cannot enable reporting, redirect it, or clear a host privacy veto', () => {
  const source = { DO_NOT_TRACK: 'true' };
  const untrusted = { DO_NOT_TRACK: 'false', LINGXI_ENABLE_TELEMETRY: 'true',
    OTEL_EXPORTER_OTLP_ENDPOINT: 'https://attacker.invalid', OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: 'https://attacker.invalid/traces',
    OTEL_RESOURCE_ATTRIBUTES: 'secret=private', OTEL_LOG_USER_PROMPTS: 'true', OTEL_EXPORTER_OTLP_HEADERS: 'authorization=private',
    LINGXI_OTEL_DIAG_STDERR: 'true', KEEP_ME: 'allowed' };
  const env = protectBridgeTelemetryEnvironment(untrusted, source, enabled);
  assert.equal(env.LINGXI_ENABLE_TELEMETRY, 'false');
  assert.equal(env.DO_NOT_TRACK, 'true');
  assert.equal(env.OTEL_EXPORTER_OTLP_ENDPOINT, 'http://127.0.0.1:4318');
  assert.equal(env.OTEL_EXPORTER_OTLP_TRACES_ENDPOINT, undefined);
  assert.equal(env.OTEL_RESOURCE_ATTRIBUTES, undefined);
  assert.equal(env.OTEL_EXPORTER_OTLP_HEADERS, '');
  assert.equal(env.OTEL_LOG_USER_PROMPTS, 'false');
  assert.equal(env.KEEP_ME, 'allowed');
  assert.equal(protectBridgeTelemetryEnvironment(untrusted, {}, resolveDesktopTelemetry({})).LINGXI_ENABLE_TELEMETRY, 'false');
});

test('host SDK exports a real request span, carries that parent on the bridge envelope, and omits content', async () => {
  const output = recorder();
  const telemetry = new DesktopTelemetry(enabled, '1.2.3', output.exporter);
  const client = new BridgeClient();
  const frames: Frame[] = [];
  (client as unknown as { ws: unknown }).ws = { readyState: 1, bufferedAmount: 0, send: (text: string) => frames.push(JSON.parse(text)) };
  const context = telemetry.beginTurn();
  assert.ok(context);
  client.sendPrompt('PRIVATE_PROMPT_MUST_NOT_APPEAR_IN_TELEMETRY', { traceContext: context });
  telemetry.onEvent({ type: 'text_delta', text: 'PRIVATE_RESPONSE_MUST_NOT_APPEAR' });
  telemetry.onEvent({ type: 'text_delta', text: 'more' });
  assert.equal(output.spans.length, 0, 'host span remains open after send');
  telemetry.endTurn('completed');
  await telemetry.forceFlush();
  assert.equal(output.spans.length, 1);
  const span = output.spans[0]!;
  assert.equal(span.name, 'lingxi.desktop.request');
  const frame = frames[0] as Extract<Frame, { type: 'request' }>;
  assert.equal(frame.payload.trace_context?.traceparent.split('-')[1], span.spanContext().traceId);
  assert.equal(frame.payload.trace_context?.traceparent.split('-')[2], span.spanContext().spanId);
  assert.equal('trace_context' in (frame.payload.params as object), false);
  assert.deepEqual(span.events.map(event => event.name), ['lingxi.first_token']);
  assert.deepEqual(span.attributes, { outcome: 'completed' });
  assert.equal(JSON.stringify({ attributes: span.attributes, events: span.events, resource: span.resource.attributes }).includes('PRIVATE_'), false);
  await telemetry.shutdown();
});

test('revocation drops queued and unfinished host spans, including shutdown flush', async () => {
  const output = recorder();
  const telemetry = new DesktopTelemetry(enabled, undefined, output.exporter);
  telemetry.beginTurn();
  telemetry.endTurn('completed');
  telemetry.beginTurn();
  telemetry.disable();
  assert.equal(telemetry.beginTurn(), undefined);
  await telemetry.forceFlush();
  await telemetry.shutdown();
  assert.equal(output.spans.length, 0);
  const off = new DesktopTelemetry(resolveDesktopTelemetry({}), undefined, output.exporter);
  assert.equal(off.beginTurn(), undefined);
  await off.shutdown();
  assert.equal(output.spans.length, 0);
});

test('invalid W3C carriers are dropped without losing the user command', () => {
  const client = new BridgeClient();
  const frames: Frame[] = [];
  (client as unknown as { ws: unknown }).ws = { readyState: 1, bufferedAmount: 0, send: (text: string) => frames.push(JSON.parse(text)) };
  client.sendPrompt('still sent', { traceContext: { traceparent: `00-${'0'.repeat(32)}-${'1'.repeat(16)}-01` } });
  const frame = frames[0] as Extract<Frame, { type: 'request' }>;
  assert.equal(frame.payload.trace_context, undefined);
  assert.equal((frame.payload.params as { text: string }).text, 'still sent');
});

test('privacy flag semantics match the Rust traffic policy, including nonempty false strings', () => {
  for (const flag of ['DISABLE_TELEMETRY', 'LINGXI_DISABLE_NONESSENTIAL_TRAFFIC', 'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC']) {
    for (const value of ['0', 'false', ' ', 'true']) {
      const source = { LINGXI_ENABLE_TELEMETRY: 'true', [flag]: value };
      assert.equal(resolveDesktopTelemetry(source).enabled, false, `${flag}=${value}`);
      assert.equal(protectBridgeTelemetryEnvironment({ [flag]: '' }, source, enabled).LINGXI_ENABLE_TELEMETRY, 'false');
    }
  }
  assert.equal(resolveDesktopTelemetry({ LINGXI_ENABLE_TELEMETRY: 'true', DO_NOT_TRACK: 'false' }).enabled, true);
});

test('queued turn roots retain their own lifetime and first-token events', async () => {
  const output = recorder();
  const telemetry = new DesktopTelemetry(enabled, undefined, output.exporter);
  const first = telemetry.beginTurn(11)!;
  telemetry.onEvent({ type: 'turn_started', turn_id: 11 });
  const second = telemetry.beginTurn(12)!;
  telemetry.onEvent({ type: 'text_delta', text: 'private-first' });
  telemetry.onEvent({ type: 'turn_ended', outcome: { type: 'end_turn' }, cost: {} } as never);
  await telemetry.forceFlush();
  assert.equal(output.spans.length, 1);
  assert.equal(output.spans[0]!.spanContext().spanId, first.traceparent.split('-')[2]);
  telemetry.onEvent({ type: 'turn_started', turn_id: 999 });
  telemetry.onEvent({ type: 'turn_ended', outcome: { type: 'end_turn' }, cost: {} } as never);
  await telemetry.forceFlush();
  assert.equal(output.spans.length, 1, 'an unrelated engine turn cannot consume the queued root');
  telemetry.onEvent({ type: 'turn_started', turn_id: 12 });
  telemetry.onEvent({ type: 'text_delta', text: 'private-second' });
  telemetry.onEvent({ type: 'turn_ended', outcome: { type: 'cancelled' }, cost: {} } as never);
  await telemetry.forceFlush();
  assert.equal(output.spans.length, 2);
  assert.equal(output.spans[1]!.spanContext().spanId, second.traceparent.split('-')[2]);
  assert.deepEqual(output.spans.map(span => span.attributes.outcome), ['completed', 'cancelled']);
  assert.deepEqual(output.spans.map(span => span.events.length), [1, 1]);
  await telemetry.shutdown();
});

test('transport interruption retires active and queued roots before a new request', async () => {
  const output = recorder();
  const telemetry = new DesktopTelemetry(enabled, undefined, output.exporter);
  telemetry.beginTurn(21);
  telemetry.beginTurn(22);
  telemetry.interrupt();
  await telemetry.forceFlush();
  assert.deepEqual(output.spans.map(span => span.attributes.outcome), ['interrupted', 'interrupted']);
  const next = telemetry.beginTurn(23)!;
  telemetry.onEvent({ type: 'turn_started', turn_id: 23 });
  telemetry.endTurn('completed');
  await telemetry.forceFlush();
  assert.equal(output.spans.length, 3);
  assert.equal(output.spans[2]!.spanContext().spanId, next.traceparent.split('-')[2]);
  await telemetry.shutdown();
});
