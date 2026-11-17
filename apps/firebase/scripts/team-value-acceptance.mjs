import { readFile, writeFile, rename, open } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { FIXTURES, FIXTURE_DIGEST, gradeWorkflows, workflowCommand } from './team-value-fixtures.mjs';
import { createRun, validateRun, validatePreflight, tick, receipt, POLICY } from './team-value-stability.mjs';

const readJson = async file => JSON.parse(await readFile(file, 'utf8'));
const json = value => `${JSON.stringify(value, null, 2)}\n`;
export async function atomicSave(path, value) {
  await writeFile(`${path}.tmp`, json(value), { mode: 0o600 });
  await rename(`${path}.tmp`, path);
}
export async function main(args = process.argv.slice(2)) {
  const [command, ...rest] = args;
  if (command === 'fixtures') { process.stdout.write(json({ fixtureDigest: FIXTURE_DIGEST, ...FIXTURES })); return; }
  if (command === 'workflow-command') {
    assert.equal(rest.length, 2, 'workflow-command F01 CONFIG.json');
    process.stdout.write(json(workflowCommand(rest[0], await readJson(rest[1]))));
    return;
  }
  if (command === 'grade-workflows') {
    assert.equal(rest.length, 2, 'grade-workflows INPUT.json RECEIPT.json');
    const result = gradeWorkflows(await readJson(rest[0]));
    await atomicSave(rest[1], result); // Does not copy source messages into the receipt.
    process.stdout.write(json(result));
    if (!result.passed) process.exitCode = 1;
    return;
  }
  assert.equal(command, 'stable', 'Commands: fixtures | workflow-command CASE CONFIG | grade-workflows INPUT RECEIPT | stable CONFIG CHECKPOINT ADAPTER.mjs');
  assert.equal(rest.length, 3, 'stable CONFIG.json CHECKPOINT.json /absolute/path/to/existing-qa-adapter.mjs');
  const [configPath, checkpointPath, adapterPath] = rest.map(p => resolve(p));
  const lockPath = `${checkpointPath}.lock`;
  const lock = await open(lockPath, 'wx', 0o600); // Prevent concurrent submissions for a checkpoint.
  const controller = new AbortController();
  const stop = () => controller.abort();
  process.once('SIGINT', stop); process.once('SIGTERM', stop);
  try {
    await lock.writeFile(json({ pid: process.pid }));
    const config = await readJson(configPath);
    // Explicit adapter is trusted QA code, loaded only on an explicit stable invocation.
    const { createAdapter } = await import(pathToFileURL(adapterPath).href);
    assert.equal(typeof createAdapter, 'function', 'Adapter must export createAdapter');
    const adapter = await createAdapter({ config, signal: controller.signal });
    for (const method of ['preflight', 'submit', 'poll']) assert.equal(typeof adapter[method], 'function', `Missing ${method}`);
    const boundedAdapter = Object.fromEntries(['preflight', 'submit', 'poll'].map(method => [method, (...params) => {
      const signal = AbortSignal.any([controller.signal, AbortSignal.timeout(55_000)]);
      // A non-cooperative adapter is rejected after 55s; it must honor signal to avoid background I/O.
      return Promise.race([adapter[method](...params.slice(0, method === 'preflight' ? 0 : 1), { signal }),
        new Promise((_, reject) => signal.addEventListener('abort', () => reject(new Error('ADAPTER_ABORTED')), { once: true }))]);
    }]));
    const verified = await boundedAdapter.preflight();
    // Persist a whitelist only. Adapter auth/config objects are never written to receipts.
    const preflight = { mode: verified.mode, isolatedQa: verified.isolatedQa,
      scope: { kind: verified.scope?.kind, id: verified.scope?.id, evidenceRef: verified.scope?.evidenceRef },
      package: { revision: verified.package?.revision, sha256: verified.package?.sha256, evidenceRef: verified.package?.evidenceRef, provenance: verified.package?.provenance },
      provider: { id: verified.provider?.id, model: verified.provider?.model, evidenceRef: verified.provider?.evidenceRef },
      quotas: { limits: Object.fromEntries(['team', 'member', 'bot'].map(k => [k, verified.quotas?.limits?.[k]])),
        remaining: Object.fromEntries(['team', 'member', 'bot'].map(k => [k, verified.quotas?.remaining?.[k]])) },
      limits: Object.fromEntries(['context_window_tokens', 'output_tokens_per_request', 'physical_dispatches'].map(k => [k, verified.limits?.[k]])) };
    let run;
    try { run = validateRun(await readJson(checkpointPath)); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    if (run) {
      validatePreflight(preflight, run.slots.filter(s => s.status === 'pending').length);
      assert.deepEqual({ scope: preflight.scope, package: preflight.package, provider: preflight.provider, limits: preflight.limits },
        { scope: run.preflight.scope, package: run.preflight.package, provider: run.preflight.provider, limits: run.preflight.limits }, 'Scope/package/provider/per-task limits drift on resume');
      assert.equal(preflight.mode, run.preflight.mode);
      assert.equal(config.startAt, run.startAt, 'Start cannot move on resume');
      assert.deepEqual({ generation: config.binding?.generation, conversationId: config.binding?.conversationId, sharedBotId: config.binding?.sharedBotId }, run.binding, 'Requester/device/conversation binding drift');
    } else {
      assert.ok(config.startAt > Date.now(), 'New windows must start in the future');
      run = createRun({ runId: config.runId, startAt: config.startAt, preflight });
      run.binding = { generation: config.binding?.generation, conversationId: config.binding?.conversationId, sharedBotId: config.binding?.sharedBotId };
      await atomicSave(checkpointPath, run);
    }
    assert.ok(Number.isSafeInteger(run.binding?.generation) && run.binding.generation > 0);
    for (const field of ['conversationId', 'sharedBotId']) assert.match(run.binding[field], /^[A-Za-z0-9_-]{8,64}$/u);
    // One host/device requester's authenticated adapter; no publish/create/quota-edit/cleanup actions.
    while (!controller.signal.aborted) {
      await tick(run, { clock: Date.now, adapter: boundedAdapter, binding: run.binding,
        save: value => atomicSave(checkpointPath, value), signal: controller.signal });
      const summary = receipt(run, Date.now());
      await atomicSave(`${checkpointPath}.receipt.json`, summary);
      if (summary.complete) { process.stdout.write(json(summary)); if (!summary.passed) process.exitCode = 1; break; }
      if (controller.signal.aborted) break;
      const now = Date.now(), pending = run.slots.filter(s => s.status === 'pending').map(s => s.scheduledAt);
      const next = Math.min(...pending.filter(at => at > now), ...(run.endAt > now ? [run.endAt] : []));
      const delay = Math.max(1, Math.min(POLICY.pollMs, next - now));
      await new Promise(resolve => {
        const done = () => { clearTimeout(timer); controller.signal.removeEventListener('abort', done); resolve(); };
        const timer = setTimeout(done, delay);
        controller.signal.addEventListener('abort', done, { once: true });
      });
    }
    await atomicSave(checkpointPath, run);
    if (controller.signal.aborted) process.exitCode = 130;
  } finally {
    controller.abort();
    process.removeListener('SIGINT', stop); process.removeListener('SIGTERM', stop);
    await lock.close();
    const { unlink } = await import('node:fs/promises');
    await unlink(lockPath); // Only this process's local concurrency lock; evidence/checkpoint retained.
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(() => { process.stderr.write('Acceptance stopped; inspect the local checkpoint and nonsecret adapter diagnostics. No error payload is printed.\n'); process.exitCode = 1; });
}
