/** Drive a running development bridge with the same host SDK/carrier as Desktop.
 * Usage: node --import ../../packages/bridge-client/node_modules/tsx/dist/loader.mjs
 *   scripts/telemetry-bridge-smoke.ts LOCKFILE [PROMPT] [--disable-and-repeat]
 * The caller owns the isolated fixture, provider and Collector processes.
 */
import { BridgeClient } from '../../../packages/bridge-client/src/client.js';
import { DesktopTelemetry } from '../src/main/telemetry.js';
import { resolveDesktopTelemetry } from '../src/main/telemetryPolicy.js';
import { setTimeout as delay } from 'node:timers/promises';

const lockfilePath = process.argv[2];
if (!lockfilePath) throw new Error('An isolated development bridge lockfile is required.');
const prompt = process.argv[3] ?? 'Run the configured telemetry smoke task.';
const config = resolveDesktopTelemetry(process.env);
const telemetry = new DesktopTelemetry(config, 'telemetry-smoke');
const client = new BridgeClient({ lockfilePath });
const roots: Array<string | null> = [];
let reportingDisabled = false;
client.on('permission', (request) => {
  let approved = false;
  if (request.kind.type === 'tool_use_confirm' && request.kind.tool_name === 'Read'
    && process.env.LINGXI_TELEMETRY_FIXTURE_FILE) {
    try {
      const input: unknown = JSON.parse(request.kind.tool_input_json);
      approved = Boolean(input && typeof input === 'object' && !Array.isArray(input)
        && (input as Record<string, unknown>).file_path === process.env.LINGXI_TELEMETRY_FIXTURE_FILE
        && Object.keys(input).every(key => key === 'file_path'));
    } catch { /* Unknown fixture requests are denied, never approved broadly. */ }
  }
  if (approved) client.approvePermission(request.request_id, { type: 'allow_once' });
  else client.denyPermission(request.request_id);
});

async function turn(id: number): Promise<void> {
  const context = telemetry.beginTurn(id);
  roots.push(context?.traceparent ?? null);
  let answerTail = '';
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => finish(new Error('Telemetry bridge smoke turn timed out.')), 60_000);
    const onError = () => finish(new Error('Telemetry bridge transport failed.'));
    const onClose = () => finish(new Error('Telemetry bridge transport closed.'));
    const onEvent = (event: Parameters<DesktopTelemetry['onEvent']>[0]) => {
      telemetry.onEvent(event);
      if (event.type === 'text_delta') answerTail = (answerTail + event.text).slice(-256);
      if (event.type === 'turn_ended') finish(answerTail.includes('TELEMETRY_AGENT_COMPLETED')
        ? undefined : new Error('Telemetry fixture did not complete its expected answer.'));
      else if (event.type === 'error') finish(new Error('Telemetry bridge reported a request error.'));
    };
    const finish = (error?: Error) => {
      clearTimeout(timer);
      client.off('event', onEvent);
      client.off('error', onError);
      client.off('close', onClose);
      if (error) { telemetry.endTurn('error', context); reject(error); }
      else resolve();
    };
    client.on('event', onEvent);
    client.on('error', onError);
    client.on('close', onClose);
    try { client.sendPrompt(prompt, { turnId: id, ...(context ? { traceContext: context } : {}) }); }
    catch { finish(new Error('Telemetry bridge prompt could not be sent.')); }
  });
}

try {
  await client.connect();
  await turn(1);
  await telemetry.forceFlush();
  // Test-only wait for the Rust batch readers; normal Desktop never waits here.
  await delay(3000);
  process.stdout.write(`${JSON.stringify({ checkpoint: 'enabled-flushed', traceparent: roots[0] })}\n`);
  if (process.argv.includes('--disable-and-repeat')) {
    telemetry.disable();
    await Promise.all([telemetry.shutdown(), client.disableTelemetry()]);
    reportingDisabled = true;
    process.stdout.write(`${JSON.stringify({ checkpoint: 'disabled', traceparent: roots[0] })}\n`);
    if (process.argv.includes('--checkpoint-stdin')) {
      await new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('Disable checkpoint acknowledgement timed out.')), 10_000);
        process.stdin.once('data', () => { clearTimeout(timer); process.stdin.pause(); resolve(); });
        process.stdin.resume();
      });
    }
    await turn(2);
    await telemetry.forceFlush();
  }
  process.stdout.write(`${JSON.stringify({ success: true, roots, reportingDisabled })}\n`);
} finally {
  client.close();
  await telemetry.shutdown();
}
