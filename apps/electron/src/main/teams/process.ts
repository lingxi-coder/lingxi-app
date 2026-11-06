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
export interface TeamHostBootstrap {
  version: 3; type: 'inspect_scope' | 'inspect_result' | 'run'; storageRoot: string; workerExecutable: string; scope: BotChatScope;
  taskId?: string; invocationId?: string; instructions?: string; inputBatchRef?: TeamDataRef;
  knowledge?: Array<{ id: string; title: string; text: string }>;
  deadlineAt?: number; model?: Omit<TeamModelRoute, 'credential_id'>; credentials?: ProviderAccountEnvelope;
  limits?: { max_turns: number; context_window_tokens: number; output_tokens_per_request: number; physical_dispatches: number };
}
export interface TeamHostResult {
  status: 'inspected' | 'not_found' | 'completed' | 'failed'; cursor: BotChatCursor; text?: string; finishedAt?: number;
  document?: { operationId: string; deliveryId: string; title: string; markdown: string; sha256: string };
  dispatchedRequests?: number; deniedTools?: number; usage?: TaskUsage;
}
export class TeamHostFailure extends Error { constructor(readonly code: string) { super('TEAM_HOST_FAILED'); } }
export interface TeamHostProcessOptions {
  executable: string; home: string; tmp: string; bootstrap: TeamHostBootstrap; signal: AbortSignal;
  assertCurrent(): void;
  accountCredentials?: (request: { accountId: string; identityGeneration: number; credentialRevision: number }) => Promise<ProviderAccountLaunch>;
  readChat?: (query: TeamChatQuery) => Promise<TeamDataRef>;
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
      if (['provider_account_refresh', 'read_chat', 'authorize'].includes(value.type as string)) {
        exact(value, value.type === 'provider_account_refresh' ? ['version','type','id','account_id','identity_generation','credential_revision'] : value.type === 'read_chat' ? ['version','type','id','query'] : ['version','type','id']);
        if (typeof value.id !== 'string') throw new Error('TEAM_HOST_PROTOCOL');
        const match = /^(provider_account_refresh|read_chat|authorize)-([1-9][0-9]{0,19})$/u.exec(value.id);
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
          } else { if (!options.authorize) throw new Error('TEAM_HOST_PROTOCOL'); await options.authorize(); send({ version: 3, type: 'authorized', id: value.id }); }
        })();
        requests.add(request); void request.catch(() => finish(new Error('TEAM_HOST_PROTOCOL'))).finally(() => requests.delete(request));
      } else if (value.type === 'result') {
        if (value.status === 'inspected' || value.status === 'not_found') exact(value,['version','type','status','cursor']);
        else exact(value,['version','type','status','cursor','text','finishedAt','dispatchedRequests','deniedTools','usage'],['document']);
        if (!['inspected','not_found','completed','failed'].includes(value.status as string) || !cursor(value.cursor)
          || (value.status === 'inspected') !== (options.bootstrap.type === 'inspect_scope') || (value.status === 'not_found' && options.bootstrap.type !== 'inspect_result')
          || (!['inspected','not_found'].includes(value.status as string) && (typeof value.text !== 'string' || Buffer.byteLength(value.text) > 8192 || !Number.isSafeInteger(value.finishedAt)))
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
        result = { status: value.status, cursor: value.cursor, ...(value.usage !== undefined ? { usage: value.usage } : {}), ...(value.text !== undefined ? { text: value.text } : {}), ...(value.document !== undefined ? { document: value.document } : {}), ...(value.finishedAt !== undefined ? { finishedAt: value.finishedAt } : {}), ...(value.dispatchedRequests !== undefined ? { dispatchedRequests: value.dispatchedRequests } : {}), ...(value.deniedTools !== undefined ? { deniedTools: value.deniedTools } : {}) } as TeamHostResult;
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
