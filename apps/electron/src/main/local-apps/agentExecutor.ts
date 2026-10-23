import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { homedir } from 'node:os';
import { createHash } from 'node:crypto';
import type { AskQuestionDto } from '@lingxi/bridge-client';
import type { LocalAppAgentCatalog, LocalAppAgentToolChoice, LocalAppAgentUsage } from '../../shared/localAppAgent.js';
import type { LocalAppAgentExecution, LocalAppAgentExecutorResult } from './agentBroker.js';
import { LocalAppAgentError, sameAgentGrant } from './agentPolicy.js';
import { buildBridgeEnvironment, buildCredentialEnvelope } from '../host-utils.js';
import { resolveServerBin } from '../bridgeDiscovery.js';

type Credentials = Parameters<typeof buildCredentialEnvelope>[0];
interface WireTool {
  serverConfigId: string; configRevision: string; rawToolName: string; definitionFingerprint: string;
  configIndex: number; label?: string; description?: string;
}
interface WireModel {
  provider_id: string; request_model: string; display_model: string;
  capabilities: { vision?: boolean; tools?: boolean; structured_output?: boolean };
}
interface WireCatalog {
  type: 'catalog'; protocolVersion: 1;
  models: WireModel[]; builtins: Array<{ name: string; description?: string }>;
  mcpTools: WireTool[]; mcpConfigs: unknown[];
  providerProfiles?: unknown; routing?: unknown;
}
interface WireEvent extends Record<string, unknown> { type: string; protocolVersion: number; runId?: string; sequence?: number }

export interface LocalAppAgentExecutorOptions {
  configHome: string;
  root: string;
  isPackaged?: boolean;
  resourcesPath?: string;
  serverBin?: string;
  /** Resolved only in the trusted host. Refresh tokens never go to the child. */
  credentials(providerId: string, refresh?: boolean): Promise<Credentials>;
  credentialStatus(providerIds: string[]): Promise<Set<string>>;
  permission(input: { appId: string; runId: string; toolName: string; input: unknown }, signal: AbortSignal): Promise<boolean>;
  question(input: { appId: string; runId: string; requestId: number; questions: AskQuestionDto[] }, signal: AbortSignal): Promise<{ answers: Array<{ question: string; answer: string }> } | null>;
  /** Test-only injected process entry. Production always resolves the packaged binary. */
  spawnProcess?: typeof spawn;
  environment?: NodeJS.ProcessEnv;
  catalogTimeoutMs?: number;
}

const PROTOCOL_VERSION = 1;
const MAX_INPUT_BYTES = 116 * 1024 * 1024;
const MAX_LINE_BYTES = 8 * 1024 * 1024;
const MAX_OUTPUT_BYTES = 64 * 1024 * 1024;
const CATALOG_TTL_MS = 30_000;
function record(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value); }
function bounded(value: unknown, limit = 512): value is string { return typeof value === 'string' && value.length > 0 && value.length <= limit && !value.includes('\0'); }
function protocolError(): LocalAppAgentError { return new LocalAppAgentError('executor_protocol', 'The Agent host returned an invalid response.'); }
function executionErrorMessage(code: string): string {
  const messages: Record<string, string> = {
    invalid_images: 'The image could not be decoded. Select a valid JPEG, PNG, WebP or GIF image.',
    invalid_schema: 'The output schema is invalid. Use a valid JSON Schema Draft 7 object.',
    model_unavailable: 'The selected Agent model is unavailable. Review this app in LingXi Apps.',
    model_selection_changed: 'The selected model is blocked by host policy. Choose an available model.',
    model_vision_unsupported: 'The selected Agent model does not support images.',
    model_structured_output_unsupported: 'The selected Agent model does not support structured results.',
    tool_changed: 'A requested tool changed. Review and renew its permission in LingXi Apps.',
    turn_limit_exceeded: 'The Agent task reached its allowed turn limit.',
    timeout: 'The Agent task reached its time limit.',
    structured_output_missing: 'The Agent did not produce a result matching the requested schema.',
    result_too_large: 'The Agent result exceeded the allowed size.',
  };
  return messages[code] ?? 'The Agent could not finish this task. Review its configuration and retry.';
}
function credentialEnvelope(value: Credentials): unknown { return JSON.parse(buildCredentialEnvelope(value)); }
function publicUsage(value: unknown): LocalAppAgentUsage | undefined {
  if (!record(value)) return undefined;
  const result: LocalAppAgentUsage = {};
  for (const [wire, key] of [['inputTokens', 'inputTokens'], ['outputTokens', 'outputTokens'], ['totalUsd', 'costUsd'], ['apiCalls', 'turns']] as const) {
    const count = value[wire];
    if (typeof count === 'number' && Number.isFinite(count) && count >= 0) result[key] = count;
  }
  return result;
}
function toolId(tool: WireTool): string {
  return `mcp:${createHash('sha256').update(JSON.stringify([tool.serverConfigId, tool.rawToolName])).digest('hex')}`;
}
function parseCatalog(value: unknown): WireCatalog {
  if (!record(value) || value.type !== 'catalog' || value.protocolVersion !== PROTOCOL_VERSION
    || !Array.isArray(value.models) || !Array.isArray(value.builtins) || !Array.isArray(value.mcpTools) || !Array.isArray(value.mcpConfigs)
    || value.models.length > 10_000 || value.mcpTools.length > 10_000 || value.mcpConfigs.length > 512) throw protocolError();
  for (const model of value.models) if (!record(model) || !bounded(model.provider_id) || !bounded(model.request_model) || !bounded(model.display_model)
    || !record(model.capabilities)) throw protocolError();
  for (const tool of value.builtins) if (!record(tool) || !bounded(tool.name)) throw protocolError();
  for (const tool of value.mcpTools) if (!record(tool) || !bounded(tool.serverConfigId) || !bounded(tool.configRevision)
    || !bounded(tool.rawToolName) || !bounded(tool.definitionFingerprint) || !Number.isSafeInteger(tool.configIndex)
    || (tool.configIndex as number) < 0 || (tool.configIndex as number) >= value.mcpConfigs.length) throw protocolError();
  return value as unknown as WireCatalog;
}

/** One private subprocess per task. No normal chat session or renderer controls are reused. */
export class LocalAppAgentExecutor {
  private cached?: { wire: WireCatalog; expires: number; configVersion: string };
  private catalogPending?: Promise<WireCatalog>;
  private credentialAvailability?: { key: string; expires: number; value: Promise<Set<string>> };
  constructor(private readonly options: LocalAppAgentExecutorOptions) {}

  invalidateCatalog(): void { this.cached = undefined; this.credentialAvailability = undefined; }
  private async configVersion(): Promise<string> {
    const environment = this.options.environment ?? process.env;
    const paths = [join(this.options.configHome, 'settings.json'), join(this.options.configHome, '.config.json'),
      join(environment.LINGXI_CONFIG_DIR ?? environment.HOME ?? homedir(), '.lingxi.json')];
    const versions = await Promise.all(paths.map(async path => {
      try { const value = await fs.stat(path, { bigint: true }); return [path, String(value.ino), String(value.size), String(value.mtimeNs), String(value.ctimeNs)]; }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return [path, 'absent']; throw error; }
    }));
    return JSON.stringify(versions);
  }
  private async wireCatalog(force = false): Promise<WireCatalog> {
    const configVersion = await this.configVersion();
    if (!force && this.cached && this.cached.expires > Date.now() && this.cached.configVersion === configVersion) return this.cached.wire;
    if (this.catalogPending) return this.catalogPending;
    this.catalogPending = (async () => {
      const cwd = join(this.options.root, 'catalog');
      await fs.mkdir(cwd, { recursive: true, mode: 0o700 });
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), this.options.catalogTimeoutMs ?? 60_000);
      try {
        let catalogVersion = configVersion;
        for (let attempt = 0; attempt < 2; attempt++) {
          let result: WireCatalog | undefined;
          await this.exchange({ type: 'catalog', protocolVersion: 1, cwd, configHome: this.options.configHome, credentials: credentialEnvelope({}) }, cwd, controller.signal, async event => {
            if (event.type !== 'catalog') throw protocolError();
            result = parseCatalog(event); return true;
          });
          if (!result) throw protocolError();
          const currentVersion = await this.configVersion();
          if (currentVersion !== catalogVersion) {
            // A fresh Harness home may initialize host metadata on first read.
            // Re-read once; never publish a catalog from a changing snapshot.
            if (attempt === 0) { catalogVersion = currentVersion; continue; }
            throw new LocalAppAgentError('configuration_changed', 'Agent configuration changed while loading. Please retry.');
          }
          this.cached = { wire: result, expires: Date.now() + CATALOG_TTL_MS, configVersion: catalogVersion };
          return result;
        }
        throw protocolError();
      } finally { clearTimeout(timer); }
    })().finally(() => { this.catalogPending = undefined; });
    return this.catalogPending;
  }
  async catalog(force = false): Promise<LocalAppAgentCatalog> {
    const wire = await this.wireCatalog(force);
    const providerIds = [...new Set(wire.models.map(model => model.provider_id))].sort();
    const key = JSON.stringify(providerIds);
    if (force || !this.credentialAvailability || this.credentialAvailability.key !== key || this.credentialAvailability.expires <= Date.now()) {
      const value = this.options.credentialStatus(providerIds);
      this.credentialAvailability = { key, expires: Date.now() + 5000, value };
      void value.catch(() => { if (this.credentialAvailability?.value === value) this.credentialAvailability = undefined; });
    }
    const available = await this.credentialAvailability.value;
    const tools: LocalAppAgentToolChoice[] = wire.builtins.map(tool => ({ id: `builtin:${tool.name}`, kind: 'builtin', name: tool.name,
      label: tool.name, ...(typeof tool.description === 'string' ? { description: tool.description } : {}) }));
    tools.push(...wire.mcpTools.map(tool => ({ id: toolId(tool), kind: 'mcp' as const, serverConfigId: tool.serverConfigId, configRevision: tool.configRevision,
      rawToolName: tool.rawToolName, definitionDigest: tool.definitionFingerprint, label: tool.label || tool.rawToolName,
      ...(typeof tool.description === 'string' ? { description: tool.description } : {}) })));
    if (new Set(tools.map(tool => tool.id)).size !== tools.length) throw protocolError();
    return { models: wire.models.map(model => ({ providerId: model.provider_id, modelId: model.request_model, label: model.display_model,
      canSelect: available.has(model.provider_id), supportsImages: model.capabilities.vision === true,
      // Harness validates StructuredOutput as a tool, independent of provider response_format support.
      supportsStructuredOutput: model.capabilities.tools === true })), tools };
  }
  async execute(execution: LocalAppAgentExecution): Promise<LocalAppAgentExecutorResult> {
    const { owner, runId, authorization, input, signal } = execution;
    await execution.assertAuthorized();
    const catalog = await this.catalog(true), wire = await this.wireCatalog();
    await execution.assertAuthorized();
    for (const grant of authorization.settings.tools) if (!catalog.tools.some(tool => sameAgentGrant(grant, tool))) {
      throw new LocalAppAgentError('tool_changed', 'A tool changed after it was authorized. Review the application permissions.');
    }
    const model = catalog.models.find(row => row.providerId === authorization.settings.model.providerId && row.modelId === authorization.settings.model.modelId);
    if (!model?.canSelect) throw new LocalAppAgentError('model_unavailable', 'The selected Agent model is not configured.');
    const credentials = await this.options.credentials(model.providerId);
    await execution.assertAuthorized();
    const mcp = authorization.settings.tools.flatMap(grant => grant.kind === 'mcp'
      ? wire.mcpTools.filter(tool => toolId(tool) === grant.id) : []);
    const mcpConfigs = [...new Set(mcp.map(tool => tool.configIndex))].map(index => wire.mcpConfigs[index]);
    const cwd = join(this.options.root, 'runs', runId);
    await fs.mkdir(cwd, { recursive: true, mode: 0o700 });
    let result: LocalAppAgentExecutorResult | undefined, sequence = 0;
    let lastProgress: string | undefined, lastProgressAt = 0;
    const progress = async (phase: Parameters<LocalAppAgentExecution['emit']>[0]['phase']) => {
      const now = Date.now();
      if (phase === lastProgress && now - lastProgressAt < 250) return;
      lastProgress = phase; lastProgressAt = now;
      await execution.emit({ phase });
    };
    const start = { type: 'start', protocolVersion: 1, runId, appId: owner.appId, versionId: String(owner.revision), invocationId: owner.invocationId,
      cwd, configHome: this.options.configHome, model: `${model.providerId}/${model.modelId}`, providerProfiles: wire.providerProfiles, routing: wire.routing,
      credentials: credentialEnvelope(credentials), mcpConfigs, toolPolicy: {
        builtinTools: authorization.settings.tools.flatMap(grant => grant.kind === 'builtin' ? [grant.name] : []),
        mcpTools: mcp.map(({ serverConfigId, configRevision, rawToolName, definitionFingerprint }) => ({ serverConfigId, configRevision, rawToolName, definitionFingerprint })),
      }, prompt: input.prompt, images: input.images ?? [], outputSchema: input.outputSchema,
      maxTurns: authorization.settings.maxTurns, timeoutMs: authorization.settings.timeoutMs };
    await this.exchange(start, cwd, signal, async (event, send, childSignal) => {
      if (event.runId !== runId || !Number.isSafeInteger(event.sequence) || event.sequence! <= sequence) throw protocolError();
      sequence = event.sequence!;
      await execution.assertAuthorized();
      if (event.type === 'started') await progress('starting');
      else if (event.type === 'text') {
        if (typeof event.delta !== 'string') throw protocolError();
        await progress('thinking');
      } else if (event.type === 'tool_started' || event.type === 'tool_finished') {
        await progress('tool');
      } else if (event.type === 'usage') {
        // The final snapshot includes authoritative usage after runtime shutdown.
        if (!record(event.usage)) throw protocolError();
      }
      else if (event.type === 'progress') {
        const phases = ['starting', 'thinking', 'tool', 'waiting_for_permission', 'waiting_for_answer'] as const;
        if (!phases.includes(event.phase as typeof phases[number])) throw protocolError();
        await progress(event.phase as typeof phases[number]);
      } else if (event.type === 'permission') {
        if (!Number.isSafeInteger(event.requestId) || !bounded(event.toolName)) throw protocolError();
        await progress('waiting_for_permission');
        const approved = await this.options.permission({ appId: owner.appId, runId, toolName: event.toolName, input: event.input }, childSignal);
        await execution.assertAuthorized();
        send({ type: 'permission_response', requestId: event.requestId, approved });
      } else if (event.type === 'question') {
        if (!Number.isSafeInteger(event.requestId) || !Array.isArray(event.questions) || event.questions.length > 8) throw protocolError();
        await progress('waiting_for_answer');
        const answers = await this.options.question({ appId: owner.appId, runId, requestId: event.requestId as number, questions: event.questions as AskQuestionDto[] }, childSignal);
        await execution.assertAuthorized();
        send(answers ? { type: 'question_response', requestId: event.requestId, answers: answers.answers }
          : { type: 'question_cancel', requestId: event.requestId });
      } else if (event.type === 'credential_required') {
        const refreshed = await this.options.credentials(model.providerId, true);
        await execution.assertAuthorized();
        send({ type: 'credential_update', credentials: credentialEnvelope(refreshed) });
      } else if (event.type === 'finished') {
        if (event.status !== 'succeeded') {
          if (event.status === 'interrupted') throw new LocalAppAgentError('interrupted', 'The Agent host connection was interrupted. This task will not be replayed.');
          const code = record(event.error) && bounded(event.error.code, 128) ? event.error.code : 'execution_failed';
          // Runtime errors may mention request data. Return a fixed message across the app boundary.
          throw new LocalAppAgentError(code, event.status === 'cancelled' ? 'The Agent task was cancelled.' : executionErrorMessage(code));
        }
        if (event.text !== undefined && typeof event.text !== 'string') throw protocolError();
        if (input.outputSchema && event.result === undefined) throw new LocalAppAgentError('invalid_result', 'The Agent did not produce a validated structured result.');
        result = { result: { ...(typeof event.text === 'string' ? { text: event.text } : {}), ...(event.result === undefined ? {} : { output: event.result }) },
          ...(record(event.usage) ? { usage: publicUsage(event.usage) } : {}) };
        return true;
      } else throw protocolError();
      return false;
    });
    if (!result) throw protocolError();
    return result;
  }

  private exchange(first: unknown, cwd: string, signal: AbortSignal, onEvent: (event: WireEvent, send: (message: unknown) => void, childSignal: AbortSignal) => Promise<boolean>): Promise<void> {
    signal.throwIfAborted();
    const line = `${JSON.stringify(first)}\n`;
    if (Buffer.byteLength(line) > MAX_INPUT_BYTES) return Promise.reject(new LocalAppAgentError('input_too_large', 'The Agent request is too large.'));
    const child = (this.options.spawnProcess ?? spawn)(resolveServerBin(this.options), ['--app-agent'], {
      cwd, env: buildBridgeEnvironment(this.options.environment ?? process.env), stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
      detached: process.platform !== 'win32',
    }) as ChildProcessWithoutNullStreams;
    return new Promise((resolve, reject) => {
      let buffer = '', bytes = 0, ended = false, completed = false, settled = false;
      let failure: unknown;
      let pending = Promise.resolve();
      let killTimer: ReturnType<typeof setTimeout> | undefined;
      let shutdownTimer: ReturnType<typeof setTimeout> | undefined;
      const childController = new AbortController();
      const kill = (which: NodeJS.Signals) => {
        if (!child.pid || ended) return;
        try { if (process.platform !== 'win32') process.kill(-child.pid, which); else child.kill(which); } catch { /* already reaped */ }
      };
      const settle = (error?: unknown) => {
        if (settled) return; settled = true;
        signal.removeEventListener('abort', abort);
        if (killTimer) clearTimeout(killTimer);
        if (shutdownTimer) clearTimeout(shutdownTimer);
        childController.abort();
        error ? reject(error) : resolve();
      };
      const stop = () => {
        if (killTimer) return;
        kill('SIGTERM'); killTimer = setTimeout(() => kill('SIGKILL'), 1000); killTimer.unref?.();
      };
      const fail = (error: unknown) => {
        if (settled) return;
        failure ??= error; childController.abort();
        if (ended) settle(failure); else stop();
      };
      const send = (message: unknown) => {
        if (ended || child.stdin.destroyed) return;
        child.stdin.write(`${JSON.stringify(message)}\n`);
      };
      const abort = () => { send({ type: 'cancel' }); fail(new LocalAppAgentError('cancelled', 'The Agent task was cancelled.')); };
      child.stdin.on('error', () => { if (!completed && !settled) fail(new LocalAppAgentError('executor_closed', 'The Agent host closed before accepting the request.')); });
      child.on('error', () => fail(new LocalAppAgentError('executor_unavailable', 'The Agent host could not start.')));
      // Never forward private subprocess diagnostics into an untrusted app or its ledger.
      child.stderr.on('data', () => undefined);
      child.stdout.setEncoding('utf8');
      child.stdout.on('data', (chunk: string) => {
        bytes += Buffer.byteLength(chunk); buffer += chunk;
        if (bytes > MAX_OUTPUT_BYTES || Buffer.byteLength(buffer) > MAX_LINE_BYTES) { fail(protocolError()); return; }
        let newline: number;
        while ((newline = buffer.indexOf('\n')) !== -1) {
          const raw = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
          pending = pending.then(async () => {
            if (settled || failure) return;
            if (completed) throw protocolError();
            const event: unknown = JSON.parse(raw);
            if (!record(event) || !bounded(event.type, 64) || event.protocolVersion !== 1) throw protocolError();
            completed = await onEvent(event as WireEvent, send, childController.signal);
            if (completed && !ended) { child.stdin.end(); shutdownTimer = setTimeout(() => fail(new LocalAppAgentError('executor_shutdown', 'The Agent host did not shut down.')), 8000); }
          }).catch(fail);
        }
      });
      child.on('close', code => {
        ended = true;
        childController.abort();
        if (failure) { settle(failure); return; }
        void pending.then(() => {
          if (settled) return;
          if (!completed || code !== 0 || buffer.trim()) settle(new LocalAppAgentError('executor_closed', 'The Agent host exited before completing the task.'));
          else settle();
        }, settle);
      });
      signal.addEventListener('abort', abort, { once: true });
      if (signal.aborted) { abort(); return; }
      child.stdin.write(line);
    });
  }
}
