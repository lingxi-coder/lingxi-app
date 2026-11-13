import { providerAccountEnvelope, type ProviderAccountEnvelope, type ProviderAccountLaunch } from '../provider-accounts.js';
import { createHash } from 'node:crypto';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { existsSync, lstatSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';
import type { TaskUsage } from '../../../../../packages/teams/execution.js';
import type { TeamDataRef, TeamModelRoute } from './privateStore.js';

export interface BotChatScope { teamId: string; sharedBotId: string; conversationId: string; sessionEpoch: number }
export interface BotChatCursor { scope: BotChatScope | null; sessionRevision: number; lastMessageSeq: number; lastChangeSeq: number; bootstrapFromSeq: number }
export type TeamChatQuery = { kind: 'before'; before_seq: number; limit: number } | { kind: 'range'; from_seq_exclusive: number; to_seq_inclusive: number } | { kind: 'by_id'; message_id: string };
/** `continue` only. A waiting task continues with the reply to exactly its wait: the answer to its question, or (`approved`) the decision on the change it proposed. */
export interface TeamContinuation { epoch: number; expected_checkpoint_digest: string; answer?: { wait_id: string; message_id: string; text: string; approved?: boolean } }
export interface TeamHostBootstrap {
  version: 3; type: 'inspect_scope' | 'inspect_result' | 'inspect_task' | 'run' | 'continue'; storageRoot: string; workerExecutable: string; scope: BotChatScope;
  taskId?: string; invocationId?: string; instructions?: string; inputBatchRef?: TeamDataRef;
  continuation?: TeamContinuation;
  /** Offer the Bot the ask-the-requester tool (`run` and `continue`). */
  askRequester?: boolean;
  /** Offer the Bot its shared list (`run` and `continue`): the title, and what the Bot is told the list is for.
   * With `approval: 'requester'` the Bot can only propose a change and the task waits for the requester's decision; this needs `askRequester`. */
  sharedList?: { title: string; description: string; approval?: 'requester' };
  knowledge?: Array<{ id: string; title: string; text: string }>;
  deadlineAt?: number; model?: Omit<TeamModelRoute, 'credential_id'>; credentials?: ProviderAccountEnvelope;
  limits?: { max_turns: number; context_window_tokens: number; output_tokens_per_request: number; physical_dispatches: number };
}
/** What a paused task is blocked on, as the runtime's durable checkpoint names it: a question, or (`tool_approval`) a proposed list change, whose `question` is that change as JSON. */
export interface TeamTaskWait { waitId: string; kind: 'human_input' | 'tool_approval'; logicalStepId: string; question: string; requestDigest: string }
export interface TeamRunResult {
  status: 'inspected' | 'not_found' | 'completed' | 'failed' | 'waiting'; cursor: BotChatCursor; text?: string; finishedAt?: number;
  document?: { operationId: string; deliveryId: string; title: string; markdown: string; sha256: string };
  /** `dispatchedRequests` counts every attempt of the task, `attemptDispatchedRequests` this process only. */
  dispatchedRequests?: number; attemptDispatchedRequests?: number; deniedTools?: number; usage?: TaskUsage;
  /** `waiting` only: the worker has exited and the task continues from this checkpoint with the answer. */
  wait?: TeamTaskWait; checkpointDigest?: string; pausedAt?: number;
}
/** `inspect_task`: what the durable session says about one task. Always inspect again before the next attempt. */
export interface TeamTaskCheckpoint {
  status: 'task_checkpoint'; taskId: string; phase: 'absent' | 'admitted' | 'in_flight' | 'result_recorded';
  attempts: number; dispatchesObserved: number; continuable: boolean; reason?: string; wait?: TeamTaskWait; checkpointDigest: string;
}
export type TeamHostResult = TeamRunResult | TeamTaskCheckpoint;
/** The result of every bootstrap except `inspect_task`. */
export function runResult(result: TeamHostResult): TeamRunResult { if (result.status === 'task_checkpoint') throw new Error('TEAM_HOST_PROTOCOL'); return result; }
const DIGEST = /^[a-f0-9]{64}$/u, count = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
const WAIT_ID = /^wait_[a-f0-9]{40}$/u;
function wait(value: unknown): value is TeamTaskWait {
  if (!object(value)) return false;
  exact(value, ['waitId', 'kind', 'logicalStepId', 'question', 'requestDigest']);
  return typeof value.waitId === 'string' && WAIT_ID.test(value.waitId) && typeof value.logicalStepId === 'string' && value.logicalStepId.length <= 256
    && (value.kind === 'human_input' || value.kind === 'tool_approval')
    && typeof value.question === 'string' && !!value.question.trim() && Buffer.byteLength(value.question) <= (value.kind === 'human_input' ? 2000 : OPERATION_BYTES) && typeof value.requestDigest === 'string' && DIGEST.test(value.requestDigest);
}
/** One request of the running Bot to its shared list. A write names the durable step it belongs to, which is its identity in the operation ledger,
 * and on a list whose changes the requester approves also the wait in which they approved it. */
export type TeamOperation = { tool: 'list_read' } | { tool: 'list_write'; logicalStepId: string; approvalRef?: string; arguments: Record<string, unknown> };
/** `established: false` says nobody knows whether the write happened: the worker then stops with the call unfinished instead of telling the model a guess. */
export interface TeamOperationOutcome { established: boolean; succeeded: boolean; result: Record<string, unknown> }
const OPERATION_BYTES = 64 * 1024;
export class TeamHostFailure extends Error { constructor(readonly code: string) { super('TEAM_HOST_FAILED'); } }
export interface TeamHostProcessOptions {
  executable: string; home: string; tmp: string; bootstrap: TeamHostBootstrap; signal: AbortSignal;
  assertCurrent(): void;
  accountCredentials?: (request: { accountId: string; identityGeneration: number; credentialRevision: number }) => Promise<ProviderAccountLaunch>;
  readChat?: (query: TeamChatQuery) => Promise<TeamDataRef>;
  operate?: (operation: TeamOperation) => Promise<TeamOperationOutcome>;
  authorize?: () => Promise<void>;
  spawnProcess?: (executable: string, args: string[], options: Parameters<typeof spawn>[2]) => ChildProcessWithoutNullStreams;
  timeoutMs?: number;
}

/** Packaged paths are fixed. Source checkout overrides require an explicit validation switch. */
export function resolveTeamBinaries(options: { isPackaged: boolean; resourcesPath?: string; environment?: NodeJS.ProcessEnv }): { host: string; worker: string } {
  const environment = options.environment ?? process.env;
  function binary(name: 'host' | 'worker'): string {
    let path: string | undefined;
    if (options.isPackaged) path = join(options.resourcesPath ?? process.resourcesPath, 'bin', `harness-team-${name}`);
    else if (environment.LINGXI_TEAM_RUNTIME_VALIDATION === '1') path = environment[`LINGXI_TEAM_${name.toUpperCase()}_BIN`];
    if (!path || !isAbsolute(path) || !existsSync(path)) throw new Error('TEAM_RUNTIME_UNAVAILABLE');
    const info = lstatSync(path);
    if (!info.isFile() || (info.mode & 0o111) === 0) throw new Error('TEAM_RUNTIME_UNAVAILABLE'); return path;
  }
  return { host: binary('host'), worker: binary('worker') };
}
const object = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
function exact(v: Record<string, unknown>, required: string[], optional: string[] = []): void {
  if (required.some(k => !(k in v)) || Object.keys(v).some(k => !required.includes(k) && !optional.includes(k))) throw new Error('TEAM_HOST_PROTOCOL');
}
function cursor(value: unknown): value is BotChatCursor {
  if (!object(value)) return false;
  exact(value, ['scope', 'sessionRevision', 'lastMessageSeq', 'lastChangeSeq', 'bootstrapFromSeq']);
  return ['sessionRevision', 'lastMessageSeq', 'lastChangeSeq', 'bootstrapFromSeq'].every(k => Number.isSafeInteger(value[k]) && (value[k] as number) >= 0);
}
/** Private major 3 only; no credential, prompt, provider output or stderr is logged. */
export async function runTeamHost(options: TeamHostProcessOptions): Promise<TeamHostResult> {
  options.assertCurrent(); if (options.signal.aborted || options.bootstrap.version !== 3) throw new Error('INVALID_SESSION');
  const bootstrap = JSON.stringify(options.bootstrap);
  if (Buffer.byteLength(bootstrap) > 4 * 1024 * 1024) throw new Error('INVALID_TEAM_SNAPSHOT');
  const child = (options.spawnProcess ?? spawn)(options.executable, [], {
    cwd: options.home, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
    env: { HOME: options.home, LINGXI_CONFIG_DIR: options.home, TMPDIR: options.tmp, PATH: '/usr/bin:/bin:/usr/sbin:/sbin', LANG: 'C', LC_ALL: 'C',
      CFFIXED_USER_HOME: options.home, __CF_USER_TEXT_ENCODING: `0x${(process.getuid?.() ?? 0).toString(16).toUpperCase()}:0x8000100:0x0` },
  }) as ChildProcessWithoutNullStreams;
  const controller = new AbortController();
  return new Promise<TeamHostResult>((resolve, reject) => {
    let completed = false, result: TeamHostResult | undefined, buffered = Buffer.alloc(0), bytes = 0, sequence = 0n;
    const requests = new Set<Promise<void>>(); let chain = Promise.resolve();
    const stop = () => {
      controller.abort(); child.stdin.destroy();
      if (child.exitCode === null) { child.kill('SIGTERM'); const kill = setTimeout(() => { if (child.exitCode === null) child.kill('SIGKILL'); }, 2000); kill.unref(); child.once('close', () => clearTimeout(kill)); }
    };
    const finish = (error?: Error) => {
      if (completed) return; completed = true; clearTimeout(timer); options.signal.removeEventListener('abort', abort); stop();
      if (error) reject(error); else if (result) resolve(result); else reject(new Error('TEAM_HOST_INTERRUPTED'));
    };
    const abort = () => finish(new Error('INVALID_SESSION'));
    const timer = setTimeout(() => finish(new Error('TEAM_HOST_INTERRUPTED')), options.timeoutMs ?? 310_000);
    const send = (value: unknown) => { if (completed || controller.signal.aborted) throw new Error('INVALID_SESSION'); options.assertCurrent(); child.stdin.write(`${JSON.stringify(value)}\n`); };
    const frame = (line: Buffer) => {
      options.assertCurrent(); const value: unknown = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(line));
      if (!object(value) || value.version !== 3 || result) throw new Error('TEAM_HOST_PROTOCOL');
      if (['provider_account_refresh', 'read_chat', 'authorize', 'operation'].includes(value.type as string)) {
        if (value.type === 'operation') exact(value, ['version','type','id','tool','arguments'], ['logicalStepId','approvalRef']);
        else exact(value, value.type === 'provider_account_refresh' ? ['version','type','id','account_id','identity_generation','credential_revision'] : value.type === 'read_chat' ? ['version','type','id','query'] : ['version','type','id']);
        if (typeof value.id !== 'string') throw new Error('TEAM_HOST_PROTOCOL');
        const match = /^(provider_account_refresh|read_chat|authorize|operation)-([1-9][0-9]{0,19})$/u.exec(value.id);
        if (!match || match[1] !== value.type || BigInt(match[2]!) <= sequence || BigInt(match[2]!) > 18446744073709551615n || requests.size >= 8) throw new Error('TEAM_HOST_PROTOCOL'); sequence = BigInt(match[2]!);
        const request = (async () => {
          if (value.type === 'provider_account_refresh') {
            if (!options.accountCredentials || !options.bootstrap.credentials || typeof value.account_id !== 'string' || !/^[a-f0-9-]{36}$/.test(value.account_id)
              || !Number.isSafeInteger(value.identity_generation) || !Number.isSafeInteger(value.credential_revision) || (value.identity_generation as number) < 1 || (value.credential_revision as number) < 0) throw new Error('TEAM_HOST_PROTOCOL');
            await options.authorize?.(); const accounts = await options.accountCredentials({ accountId: value.account_id, identityGeneration: value.identity_generation as number, credentialRevision: value.credential_revision as number });
            send({ version: 3, type: 'provider_accounts_updated', id: value.id, credentials: providerAccountEnvelope(accounts, options.bootstrap.credentials.provider_account_profiles) });
          } else if (value.type === 'read_chat') {
            if (!options.readChat || !object(value.query)) throw new Error('TEAM_HOST_PROTOCOL');
            const q = value.query;
            if (q.kind === 'before') exact(q, ['kind', 'before_seq', 'limit']); else if (q.kind === 'range') exact(q, ['kind', 'from_seq_exclusive', 'to_seq_inclusive']); else if (q.kind === 'by_id') exact(q, ['kind', 'message_id']); else throw new Error('TEAM_HOST_PROTOCOL');
            await options.authorize?.(); const reference = await options.readChat(q as unknown as TeamChatQuery); send({ version: 3, type: 'chat_read', id: value.id, reference });
          } else if (value.type === 'operation') {
            const write = value.tool === 'list_write', step = value.logicalStepId, approval = value.approvalRef, taskId = options.bootstrap.taskId, list = options.bootstrap.sharedList;
            // A write on an approved list names its approval, and no other request does.
            if (!options.operate || !list || !taskId || !object(value.arguments)
              || (write ? typeof step !== 'string' || !step.startsWith(`${taskId}/t`) || !/^\/t[0-9]{1,10}\/c[0-9]{1,10}$/u.test(step.slice(taskId.length))
                  || (list.approval === 'requester' ? typeof approval !== 'string' || !WAIT_ID.test(approval) : approval !== undefined)
                : value.tool !== 'list_read' || step !== undefined || approval !== undefined || Object.keys(value.arguments).length > 0)) throw new Error('TEAM_HOST_PROTOCOL');
            await options.authorize?.();
            const outcome = await options.operate(write ? { tool: 'list_write', logicalStepId: step as string, ...(typeof approval === 'string' ? { approvalRef: approval } : {}), arguments: value.arguments } : { tool: 'list_read' });
            if (!object(outcome.result) || Buffer.byteLength(JSON.stringify(outcome.result)) > OPERATION_BYTES) throw new Error('TEAM_HOST_PROTOCOL');
            send({ version: 3, type: 'operation_result', id: value.id, established: outcome.established, succeeded: outcome.established && outcome.succeeded, result: outcome.established ? outcome.result : {} });
          } else { if (!options.authorize) throw new Error('TEAM_HOST_PROTOCOL'); await options.authorize(); send({ version: 3, type: 'authorized', id: value.id }); }
        })();
        requests.add(request); void request.catch(() => finish(new Error('TEAM_HOST_PROTOCOL'))).finally(() => requests.delete(request));
      } else if (value.type === 'result') {
        const kind = options.bootstrap.type, allowed = kind === 'inspect_scope' ? ['inspected'] : kind === 'inspect_result' ? ['not_found','completed','failed'] : kind === 'inspect_task' ? ['task_checkpoint']
          : options.bootstrap.askRequester === true ? ['completed','failed','waiting'] : ['completed','failed'];
        if (typeof value.status !== 'string' || !allowed.includes(value.status)) throw new Error('TEAM_HOST_PROTOCOL');
        if (value.status === 'task_checkpoint') {
          exact(value,['version','type','status','taskId','phase','cursor','attempts','dispatchesObserved','pendingToolCalls','continuable','checkpointDigest'],['reason','wait']);
          if (value.taskId !== options.bootstrap.taskId || !['absent','admitted','in_flight','result_recorded'].includes(value.phase as string) || !count(value.attempts) || !count(value.dispatchesObserved)
            || typeof value.continuable !== 'boolean' || typeof value.checkpointDigest !== 'string' || !DIGEST.test(value.checkpointDigest) || !Array.isArray(value.pendingToolCalls) || !object(value.cursor)
            || value.reason !== undefined && (typeof value.reason !== 'string' || !/^[a-z_]{3,60}$/u.test(value.reason)) || value.wait !== undefined && !wait(value.wait)) throw new Error('TEAM_HOST_PROTOCOL');
          result = { status: 'task_checkpoint', taskId: value.taskId as string, phase: value.phase as TeamTaskCheckpoint['phase'], attempts: value.attempts, dispatchesObserved: value.dispatchesObserved, continuable: value.continuable,
            checkpointDigest: value.checkpointDigest, ...(value.reason !== undefined ? { reason: value.reason as string } : {}), ...(value.wait !== undefined ? { wait: value.wait as TeamTaskWait } : {}) };
          return;
        }
        if (value.status === 'inspected' || value.status === 'not_found') exact(value,['version','type','status','cursor']);
        else if (value.status === 'waiting') {
          exact(value,['version','type','status','cursor','wait','checkpointDigest','pausedAt','dispatchedRequests','attemptDispatchedRequests','deniedTools','usage']);
          if (!wait(value.wait) || typeof value.checkpointDigest !== 'string' || !DIGEST.test(value.checkpointDigest) || !count(value.pausedAt) || !count(value.dispatchedRequests)) throw new Error('TEAM_HOST_PROTOCOL');
        }
        // A result recorded by an earlier runtime has no per-attempt count.
        else exact(value,['version','type','status','cursor','text','finishedAt','dispatchedRequests','deniedTools','usage'],['document','attemptDispatchedRequests']);
        if (!cursor(value.cursor)
          || (['completed','failed'].includes(value.status) && (typeof value.text !== 'string' || Buffer.byteLength(value.text) > 8192 || !Number.isSafeInteger(value.finishedAt)))
          || (value.document !== undefined && (!object(value.document) || typeof value.document.markdown !== 'string' || Buffer.byteLength(value.document.markdown) > 256 * 1024))) throw new Error('TEAM_HOST_PROTOCOL');
        if (value.cursor.sessionRevision === 0 ? value.cursor.scope !== null : JSON.stringify(value.cursor.scope) !== JSON.stringify(options.bootstrap.scope)) throw new Error('TEAM_HOST_PROTOCOL');
        if (value.document !== undefined) {
          const d = value.document as Record<string,unknown>; exact(d,['operationId','deliveryId','title','markdown','sha256']);
          if (typeof d.title !== 'string' || !d.title.trim() || d.title.length>120 || typeof d.markdown !== 'string'
            || ![d.operationId,d.deliveryId].every(id => typeof id === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(id))
            || d.sha256 !== createHash('sha256').update(d.markdown).digest('hex')) throw new Error('TEAM_HOST_PROTOCOL');
        }
        if (value.usage !== undefined) {
          const usage = value.usage; if (!object(usage)) throw new Error('TEAM_HOST_PROTOCOL');
          if (usage.status === 'missing') exact(usage,['status']);
          else { exact(usage,['status','inputTokens','outputTokens'],['costMicros','currency']);
            if (!['partial','complete'].includes(usage.status as string) || !['inputTokens','outputTokens'].every(k=>Number.isSafeInteger(usage[k]) && (usage[k] as number)>=0)
              || usage.costMicros !== undefined && (!Number.isSafeInteger(usage.costMicros) || (usage.costMicros as number)<0)
              || usage.currency !== undefined && (typeof usage.currency !== 'string' || !/^[A-Z]{3}$/u.test(usage.currency))) throw new Error('TEAM_HOST_PROTOCOL'); }
        }
        result = { status: value.status, cursor: value.cursor, ...(value.usage !== undefined ? { usage: value.usage } : {}), ...(value.text !== undefined ? { text: value.text } : {}), ...(value.document !== undefined ? { document: value.document } : {}), ...(value.finishedAt !== undefined ? { finishedAt: value.finishedAt } : {}), ...(value.dispatchedRequests !== undefined ? { dispatchedRequests: value.dispatchedRequests } : {}),
          ...(value.attemptDispatchedRequests !== undefined ? { attemptDispatchedRequests: value.attemptDispatchedRequests } : {}), ...(value.deniedTools !== undefined ? { deniedTools: value.deniedTools } : {}),
          ...(value.status === 'waiting' ? { wait: value.wait, checkpointDigest: value.checkpointDigest, pausedAt: value.pausedAt } : {}) } as TeamRunResult;
      } else if (value.type === 'error') {
        exact(value, ['version','type','code']); if (typeof value.code !== 'string' || !/^[A-Z_]{3,60}$/u.test(value.code)) throw new Error('TEAM_HOST_PROTOCOL'); throw new TeamHostFailure(value.code);
      } else throw new Error('TEAM_HOST_PROTOCOL');
    };
    child.stdout.on('data', (data: Buffer) => {
      bytes += data.length; if (bytes > 4 * 1024 * 1024) { finish(new Error('TEAM_HOST_PROTOCOL')); return; }
      buffered = Buffer.concat([buffered, data]);
      for (;;) { const newline = buffered.indexOf(10); if (newline < 0) break; const line = buffered.subarray(0, newline); buffered = buffered.subarray(newline + 1);
        if (!line.length || line.length > 512 * 1024) { finish(new Error('TEAM_HOST_PROTOCOL')); return; }
        chain = chain.then(() => frame(line)); void chain.catch(error => finish(error instanceof TeamHostFailure ? error : new Error('TEAM_HOST_PROTOCOL'))); }
      if (buffered.length > 512 * 1024) finish(new Error('TEAM_HOST_PROTOCOL'));
    });
    child.stderr.on('data', (data: Buffer) => { bytes += data.length; if (bytes > 4 * 1024 * 1024) finish(new Error('TEAM_HOST_PROTOCOL')); });
    child.once('error', () => finish(new Error('TEAM_HOST_INTERRUPTED'))); child.stdin.on('error', () => finish(new Error('TEAM_HOST_INTERRUPTED')));
    child.once('close', code => { void chain.then(() => { if (buffered.length || code !== 0) finish(new Error('TEAM_HOST_INTERRUPTED')); else finish(); }, () => finish(new Error('TEAM_HOST_PROTOCOL'))); });
    options.signal.addEventListener('abort', abort, { once: true }); try { send(JSON.parse(bootstrap)); } catch { finish(new Error('INVALID_SESSION')); }
  });
}
