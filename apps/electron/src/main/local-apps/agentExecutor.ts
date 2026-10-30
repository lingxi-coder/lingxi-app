import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { homedir } from 'node:os';
import { createHash } from 'node:crypto';
import type { AskQuestionDto } from '@lingxi/bridge-client';
import type { LocalAppAgentCatalog, LocalAppAgentToolChoice, LocalAppAgentUsage } from '../../shared/localAppAgent.js';
import type { LocalAppAgentExecution, LocalAppAgentExecutorResult } from './agentBroker.js';
import { validChatCheckpoint } from './agentBroker.js';
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
  /** Test-only shortened cancellation drain window. Production uses 12 seconds. */
  cancelDrainGraceMs?: number;
}

const PROTOCOL_VERSION = 1;
const MAX_INPUT_BYTES = 116 * 1024 * 1024;
const MAX_LINE_BYTES = 8 * 1024 * 1024;
const MAX_OUTPUT_BYTES = 64 * 1024 * 1024;
const CATALOG_TTL_MS = 30_000;
// Native owns a ten-second cancellation drain, including the eight-second
// Harness shutdown barrier. Leave headroom for the control pipe and exit.
const CANCEL_DRAIN_GRACE_MS = 12_000;
function record(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value); }
function bounded(value: unknown, limit = 512): value is string { return typeof value === 'string' && value.length > 0 && value.length <= limit && !value.includes('\0'); }
function protocolError(): LocalAppAgentError { return new LocalAppAgentError('executor_protocol', 'The Agent host returned an invalid response.'); }
function executionErrorMessage(code: string): string {
  const messages: Record<string, string> = {
    invalid_attachments: 'The selected file could not be decoded or is not supported.',
    attachment_type_unsupported: 'This attachment type is not supported. Choose a text, PDF, JPEG, PNG, WebP or GIF file.',
    invalid_schema: 'The output schema is invalid. Use a valid JSON Schema Draft 7 object.',
    model_unavailable: 'The selected Agent model is unavailable. Review this app in LingXi Apps.',
    model_selection_changed: 'The selected model is blocked by host policy. Choose an available model.',
    model_vision_unsupported: 'The selected Agent model does not support images.',
    model_structured_output_unsupported: 'The selected Agent model does not support structured results.',
    tool_changed: 'A requested tool changed. Review and renew its permission in LingXi Apps.',
    turn_limit_exceeded: 'The Agent task reached its allowed turn limit.',
    timeout: 'The Agent task reached its time limit.',
    structured_output_missing: 'The Agent did not produce a result matching the requested schema.',
    chat_tool_unsupported: 'A requested tool cannot safely preserve this Chat. Review the application tool selection.',
    chat_context_unavailable: 'This Chat cannot continue until its saved context is verified. Its history and application data have been preserved.',
    chat_context_compaction_unsupported: 'This Chat history cannot be summarized safely. Its history and application data have been preserved.',
    chat_context_limit_exceeded: 'This Chat exceeds the selected model context limit. Its history and application data have been preserved.',
    chat_context_budget_unknown: 'The selected model context limit could not be verified. Review the model configuration and retry.',
    chat_context_authority_revoked: 'Application access changed while preparing this Chat. Its history and application data have been preserved.',
    tool_cleanup_incomplete: 'The Agent could not confirm that its tools stopped. Its result was not saved.',
    result_too_large: 'The Agent result exceeded the allowed size.',
  };
  return messages[code] ?? 'The Agent could not finish this task. Review its configuration and retry.';
}
function credentialEnvelope(value: Credentials): unknown { return JSON.parse(buildCredentialEnvelope(value)); }
function publicUsage(value: unknown): LocalAppAgentUsage | undefined {
  if (value === undefined) return undefined;
  if (!record(value)) throw protocolError();
  const result: LocalAppAgentUsage = {};
  for (const [wire, key] of [['inputTokens', 'inputTokens'], ['outputTokens', 'outputTokens'], ['costUsd', 'costUsd'], ['turns', 'turns']] as const) {
    const count = value[wire];
    if (count !== undefined) {
      if (typeof count !== 'number' || !Number.isFinite(count) || count < 0) throw protocolError();
      result[key] = count;
    }
  }
  if (value.costEstimated !== undefined) {
    if (typeof value.costEstimated !== 'boolean') throw protocolError();
    result.costEstimated = value.costEstimated;
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
      canSelect: available.has(model.provider_id), supportsAttachments: true, supportsVision: model.capabilities.vision === true,
      // Harness validates StructuredOutput as a tool, independent of provider response_format support.
      supportsStructuredOutput: model.capabilities.tools === true })), tools, skills: [] };
  }
  async execute(execution: LocalAppAgentExecution): Promise<LocalAppAgentExecutorResult> {
    const { owner, runId, authorization, input, signal } = execution;
    await execution.assertAuthorized();
    if (execution.chat && (execution.chat.generationId !== execution.taskStorageId
      || execution.chat.nativeUserMessageId !== `msg:${execution.chat.generationId}`
      || !/^[a-f0-9]{64}$/.test(execution.chat.cleanupNonce))) throw protocolError();
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
    const taskDirectory = join(this.options.root, 'apps', owner.appId, 'agent', 'agui-v1', 'tasks', execution.taskStorageId);
    const cwd = execution.chat ? join(this.options.root, 'apps', owner.appId, 'agent', 'agui-v1', 'workspaces', execution.chat.chatId) : taskDirectory;
    const store = join(this.options.root, 'apps', owner.appId, 'agent', 'agui-v1');
    const paths = [this.options.root, join(this.options.root, 'apps'), join(this.options.root, 'apps', owner.appId),
      join(this.options.root, 'apps', owner.appId, 'agent'), store, join(store, 'tasks'), taskDirectory, join(store, 'assets'),
      ...(execution.chat ? [join(store, 'workspaces'), cwd] : [])];
    for (const path of paths) {
      try { await fs.mkdir(path, { mode: 0o700 }); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error; }
      const stat = await fs.lstat(path);
      if (!stat.isDirectory() || stat.isSymbolicLink()) throw new LocalAppAgentError('agent_storage_unavailable', 'The Agent working directory is unsafe.');
    }
    const requestedSkills = new Set(input.skills ?? []);
    const releaseFiles = authorization.releaseFiles.filter(file => [...requestedSkills].some(id => file.path.startsWith(`skills/${id}/`)));
    let result: LocalAppAgentExecutorResult | undefined;
    let lastProgress: string | undefined, lastProgressAt = 0;
    const progress = async (phase: Parameters<LocalAppAgentExecution['emit']>[0]['phase']) => {
      const now = Date.now();
      if (phase === lastProgress && now - lastProgressAt < 250) return;
      lastProgress = phase; lastProgressAt = now;
      await execution.emit({ phase });
    };
    const start = { type: 'start', protocolVersion: 1, runId, appId: owner.appId, versionId: owner.releaseDigest, invocationId: owner.invocationId,
      cwd, configHome: this.options.configHome, model: `${model.providerId}/${model.modelId}`, providerProfiles: wire.providerProfiles, routing: wire.routing,
      credentials: credentialEnvelope(credentials), mcpConfigs, toolPolicy: {
        builtinTools: authorization.settings.tools.flatMap(grant => grant.kind === 'builtin' ? [grant.name] : []),
        mcpTools: mcp.map(({ serverConfigId, configRevision, rawToolName, definitionFingerprint }) => ({ serverConfigId, configRevision, rawToolName, definitionFingerprint })),
      }, input: { ...input, contentParts: input.contentParts.map(part => part.kind === 'text'
        ? { kind: 'text', text: part.text }
        : { kind: 'attachment', name: part.name, mediaType: part.mediaType, base64: part.base64 }) },
      ...(execution.chat ? { appChat: execution.chat } : {}),
      skillSourceDirectory: authorization.releaseSourceDirectory, releaseRevision: authorization.releaseRevision,
      skillSpecs: authorization.skillSpecs, grantedSkills: authorization.settings.skills, releaseFiles,
      modelCapabilities: { vision: model.supportsVision, structuredOutput: model.supportsStructuredOutput },
      maxTurns: authorization.settings.maxTurns, timeoutMs: authorization.settings.timeoutMs };
    await execution.assertAuthorized();
    if (execution.chat || input.tools.length || input.skills.length) await execution.requireCleanupConfirmation();
    await execution.assertAuthorized();
    await this.exchange(start, cwd, signal, async (event, send, childSignal) => {
      await execution.assertAuthorized();
      if (event.type === 'started') await progress('starting');
      else if (event.type === 'text') {
        if (typeof event.delta !== 'string') throw protocolError();
        await progress('thinking');
      } else if (event.type === 'tool_started' || event.type === 'tool_finished') {
        await progress('tool');
      } else if (event.type === 'usage') {
        if (!record(event.usage)) throw protocolError();
        const usage = publicUsage(event.usage);
        await execution.emit({ phase: 'thinking', usage });
      }
      else if (event.type === 'progress') {
        const phases = ['starting', 'thinking', 'tool', 'waiting_for_permission', 'waiting_for_answer'] as const;
        if (!phases.includes(event.phase as typeof phases[number])) throw protocolError();
        const completedModelTurns = event.completedModelTurns;
        if (completedModelTurns !== undefined && (typeof completedModelTurns !== 'number' || !Number.isSafeInteger(completedModelTurns) || completedModelTurns < 0)) throw protocolError();
        await execution.emit({ phase: event.phase as typeof phases[number], ...(completedModelTurns === undefined ? {} : { completedModelTurns }) });
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
        if (input.outputSchema && event.result === null) throw new LocalAppAgentError('structured_output_not_representable', 'The structured result cannot be represented as an AG-UI run result.');
        if (typeof event.messageId !== 'string' || !bounded(event.messageId, 256)
          || (execution.chat && event.chatId !== execution.chat.chatId)) throw protocolError();
        if (execution.chat) {
          if (!validChatCheckpoint(event.chatCheckpoint) || event.chatCheckpoint.generationId !== execution.chat.generationId
            || event.chatCheckpoint.sessionId !== execution.chat.sessionId) throw protocolError();
        } else if (event.chatCheckpoint !== undefined) throw protocolError();
        const usage = publicUsage(event.usage);
        result = { result: { text: typeof event.text === 'string' ? event.text : '', attachments: [], messageId: event.messageId,
          ...(execution.chat ? { chatId: execution.chat.chatId } : {}), ...(event.result === undefined || event.result === null ? {} : { output: event.result }) },
          ...(usage === undefined ? {} : { usage }),
          ...(execution.chat ? { chatCheckpoint: structuredClone(event.chatCheckpoint) } : {}) };
        return true;
      } else throw protocolError();
      return false;
    });
    if (!result) throw protocolError();
    return result;
  }

  private exchange(first: unknown, cwd: string, signal: AbortSignal, onEvent: (event: WireEvent, send: (message: unknown) => void, childSignal: AbortSignal) => Promise<boolean>): Promise<void> {
    signal.throwIfAborted();
    const task = record(first) && first.type === 'start' ? first : undefined;
    const taskInput = record(task?.input) ? task.input : undefined;
    const needsRuntimeCleanup = !!task?.appChat || (Array.isArray(taskInput?.tools) && taskInput.tools.length > 0)
      || (Array.isArray(taskInput?.skills) && taskInput.skills.length > 0);
    const line = `${JSON.stringify(first)}\n`;
    if (Buffer.byteLength(line) > MAX_INPUT_BYTES) return Promise.reject(new LocalAppAgentError('input_too_large', 'The Agent request is too large.'));
    const child = (this.options.spawnProcess ?? spawn)(resolveServerBin(this.options), ['--app-agent'], {
      cwd, env: buildBridgeEnvironment({ PYTHONDONTWRITEBYTECODE: '1', ...(this.options.environment ?? process.env) }), stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
      detached: process.platform !== 'win32',
    }) as ChildProcessWithoutNullStreams;
    return new Promise((resolve, reject) => {
      let buffer = '', bytes = 0, ended = false, completed = false, settled = false;
      let failure: unknown, failed = false, wireSequence = 0, terminalSeen = false;
      let cleanupAcknowledged = !needsRuntimeCleanup, cleanupUnconfirmed = false, invalidTransport = false;
      let pending = Promise.resolve();
      let killTimer: ReturnType<typeof setTimeout> | undefined;
      let shutdownTimer: ReturnType<typeof setTimeout> | undefined;
      let cancelTimer: ReturnType<typeof setTimeout> | undefined;
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
        if (cancelTimer) clearTimeout(cancelTimer);
        childController.abort();
        if (task && child.pid && (cleanupUnconfirmed || (needsRuntimeCleanup && (!cleanupAcknowledged || invalidTransport)))) {
          error = new LocalAppAgentError('tool_cleanup_incomplete', executionErrorMessage('tool_cleanup_incomplete'));
        }
        error ? reject(error) : resolve();
      };
      const stop = () => {
        if (killTimer) return;
        kill('SIGTERM'); killTimer = setTimeout(() => kill('SIGKILL'), 1000); killTimer.unref?.();
      };
      const fail = (error: unknown, drain = false) => {
        // Permission/credential callbacks may reject as a consequence of abort.
        // They must not replace cancellation's already-owned drain window.
        if (settled || failed) return;
        failed = true; failure = error; childController.abort();
        if (ended) settle(failure);
        else if (drain) {
          if (shutdownTimer) { clearTimeout(shutdownTimer); shutdownTimer = undefined; }
          const grace = this.options.cancelDrainGraceMs ?? CANCEL_DRAIN_GRACE_MS;
          cancelTimer = setTimeout(stop, Math.max(1, Math.min(CANCEL_DRAIN_GRACE_MS, grace)));
          cancelTimer.unref?.();
        } else stop();
      };
      const send = (message: unknown) => {
        if (ended || child.stdin.destroyed || child.stdin.writableEnded) return;
        child.stdin.write(`${JSON.stringify(message)}\n`);
      };
      const abort = () => {
        send({ type: 'cancel' });
        fail(new LocalAppAgentError('cancelled', 'The Agent task was cancelled.'), record(first) && first.type === 'start');
      };
      child.stdin.on('error', () => { if (!completed && !settled) fail(new LocalAppAgentError('executor_closed', 'The Agent host closed before accepting the request.')); });
      child.on('error', () => fail(new LocalAppAgentError('executor_unavailable', 'The Agent host could not start.')));
      // Never forward private subprocess diagnostics into an untrusted app or its ledger.
      child.stderr.on('data', () => undefined);
      child.stdout.setEncoding('utf8');
      child.stdout.on('data', (chunk: string) => {
        bytes += Buffer.byteLength(chunk); buffer += chunk;
        if (bytes > MAX_OUTPUT_BYTES || Buffer.byteLength(buffer) > MAX_LINE_BYTES) { invalidTransport = true; fail(protocolError()); return; }
        let newline: number;
        while ((newline = buffer.indexOf('\n')) !== -1) {
          const raw = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
          let event: WireEvent;
          try {
            const value: unknown = JSON.parse(raw);
            if (!record(value) || !bounded(value.type, 64) || value.protocolVersion !== 1 || terminalSeen || completed) throw protocolError();
            event = value as WireEvent;
            if (task) {
              if (event.runId !== task.runId || !Number.isSafeInteger(event.sequence) || event.sequence! <= wireSequence) throw protocolError();
              wireSequence = event.sequence!;
              if (event.type === 'finished') {
                if (!['succeeded', 'failed', 'cancelled', 'interrupted'].includes(event.status as string)) throw protocolError();
                terminalSeen = true;
                cleanupUnconfirmed = event.cleanupStatus === 'unconfirmed';
                cleanupAcknowledged = event.cleanupStatus === 'complete'
                  || (event.cleanupStatus === 'not_required' && (!needsRuntimeCleanup || event.status !== 'succeeded'));
                // Cleanup is a transport fact, read even after authorization or
                // UI callbacks were cancelled. It never publishes the result.
                if (failed && !child.stdin.destroyed && !child.stdin.writableEnded) child.stdin.end();
              }
            }
          } catch {
            invalidTransport = true; fail(protocolError()); return;
          }
          pending = pending.then(async () => {
            if (settled || failed) return;
            completed = await onEvent(event, send, childController.signal);
            if (completed && !ended) { child.stdin.end(); shutdownTimer = setTimeout(() => fail(new LocalAppAgentError('executor_shutdown', 'The Agent host did not shut down.')), 8000); }
          }).catch(fail);
        }
      });
      child.on('close', code => {
        ended = true;
        childController.abort();
        if (buffer.trim()) invalidTransport = true;
        if (failed) { settle(failure); return; }
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
