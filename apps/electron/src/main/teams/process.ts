import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { existsSync, lstatSync } from 'node:fs';
import { isAbsolute, join } from 'node:path';

export interface TeamBotTarget {
  ownerUid: string; ownerGeneration: number; hostId: string; botId: string; botRevision: number;
  teamId: string; conversationId: string; botKey: string; triggerId: string;
}
export interface TeamRunSnapshot {
  instructions: string; model_provider: string; model_protocol: string; model_id: string; model_endpoint: string;
  model_profile: Record<string, unknown>; model_api_key: string;
  max_turns: number; max_input_bytes: number; output_tokens_per_request: number; physical_dispatches: number;
}
export interface TeamHostBootstrap {
  version: 1; type: 'start'; mode: 'run' | 'retry_result'; owner_uid: string; account_epoch: number;
  endpoint: 'https://lingxios.net/v1/teams/messages'; journal_root: string; runtime_root: string;
  worker_executable: string; target: TeamBotTarget; snapshot?: TeamRunSnapshot;
}
export interface TeamHostResult { status: 'settled' | 'busy' | 'recovery_required' | 'unstarted'; state?: string; reply_id?: string }
export class TeamHostFailure extends Error {
  constructor(readonly code: string) { super('TEAM_HOST_FAILED'); }
}
export interface TeamHostProcessOptions {
  executable: string; home: string; tmp: string; bootstrap: TeamHostBootstrap; signal: AbortSignal;
  token(): Promise<string>; assertCurrent(): void;
  permission(input: { target: TeamBotTarget; toolName: string; input: unknown }, signal: AbortSignal): Promise<boolean>;
  spawnProcess?: (executable: string, args: string[], options: Parameters<typeof spawn>[2]) => ChildProcessWithoutNullStreams;
  timeoutMs?: number;
}

/** Packaged paths are fixed. Source checkout overrides require an explicit validation switch. */
export function resolveTeamBinaries(options: { isPackaged: boolean; resourcesPath?: string; environment?: NodeJS.ProcessEnv }): { host: string; worker: string } {
  const environment = options.environment ?? process.env;
  const binary = (name: 'host' | 'worker') => {
    const path = options.isPackaged ? join(options.resourcesPath ?? process.resourcesPath, 'bin', `harness-team-${name}`)
      : environment.LINGXI_TEAM_RUNTIME_VALIDATION === '1' ? environment[`LINGXI_TEAM_${name.toUpperCase()}_BIN`] : undefined;
    if (!path || !isAbsolute(path) || !existsSync(path)) throw new Error('TEAM_RUNTIME_UNAVAILABLE');
    const info = lstatSync(path);
    if (!info.isFile() || (info.mode & 0o111) === 0) throw new Error('TEAM_RUNTIME_UNAVAILABLE');
    return path;
  };
  return { host: binary('host'), worker: binary('worker') };
}

const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function exact(value: Record<string, unknown>, required: string[], optional: string[] = []): void {
  if (required.some(key => !(key in value)) || Object.keys(value).some(key => !required.includes(key) && !optional.includes(key))) throw new Error('TEAM_HOST_PROTOCOL');
}
const identifier = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(value);

/** Anonymous JSONL pipes only; neither credentials nor untrusted output enter logs. */
export async function runTeamHost(options: TeamHostProcessOptions): Promise<TeamHostResult> {
  options.assertCurrent();
  if (options.signal.aborted) throw new Error('INVALID_SESSION');
  const bootstrap = JSON.stringify(options.bootstrap);
  if (Buffer.byteLength(bootstrap) > 256 * 1024) throw new Error('INVALID_TEAM_SNAPSHOT');
  const child = (options.spawnProcess ?? spawn)(options.executable, [], {
    cwd: options.home, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
    env: { HOME: options.home, LINGXI_CONFIG_DIR: options.home, TMPDIR: options.tmp,
      PATH: '/usr/bin:/bin:/usr/sbin:/sbin', LANG: 'C', LC_ALL: 'C',
      CFFIXED_USER_HOME: options.home, __CF_USER_TEXT_ENCODING: `0x${(process.getuid?.() ?? 0).toString(16).toUpperCase()}:0x8000100:0x0` },
  }) as ChildProcessWithoutNullStreams;
  const controller = new AbortController();
  return new Promise<TeamHostResult>((resolve, reject) => {
    let completed = false, result: TeamHostResult | undefined, buffered = Buffer.alloc(0), bytes = 0;
    let requestSequence = 0n;
    const requests = new Set<Promise<void>>();
    let chain = Promise.resolve();
    const stop = () => {
      controller.abort(); child.stdin.destroy();
      if (child.exitCode === null) {
        child.kill('SIGTERM');
        const kill = setTimeout(() => { if (child.exitCode === null) child.kill('SIGKILL'); }, 2000);
        kill.unref(); child.once('close', () => clearTimeout(kill));
      }
    };
    const finish = (error?: Error) => {
      if (completed) return;
      completed = true; clearTimeout(timer); options.signal.removeEventListener('abort', abort); stop();
      if (error) reject(error); else if (result) resolve(result); else reject(new Error('TEAM_HOST_RECOVERY_REQUIRED'));
    };
    const abort = () => finish(new Error('INVALID_SESSION'));
    const timer = setTimeout(() => finish(new Error('TEAM_HOST_RECOVERY_REQUIRED')), options.timeoutMs ?? 310_000);
    const send = (value: unknown) => {
      if (completed || controller.signal.aborted) throw new Error('INVALID_SESSION');
      options.assertCurrent(); child.stdin.write(`${JSON.stringify(value)}\n`);
    };
    const frame = (line: Buffer) => {
      if (completed) return;
      options.assertCurrent();
      const value: unknown = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(line));
      if (!object(value) || value.version !== 1 || result) throw new Error('TEAM_HOST_PROTOCOL');
      if (value.type === 'credential' || value.type === 'permission') {
        exact(value, value.type === 'credential' ? ['version', 'type', 'id'] : ['version', 'type', 'id', 'tool_name', 'input']);
        if (typeof value.id !== 'string') throw new Error('TEAM_HOST_PROTOCOL');
        const correlation = /^(credential|permission)-([1-9][0-9]{0,19})$/u.exec(value.id);
        if (!correlation || correlation[1] !== value.type) throw new Error('TEAM_HOST_PROTOCOL');
        const sequence = BigInt(correlation[2]!);
        // Rust assigns its global counter while holding the output lock. A
        // high-water mark therefore rejects replay with constant memory and no
        // lifetime request cap during a full five-minute human-question wait.
        if (sequence <= requestSequence || sequence > 18_446_744_073_709_551_615n) throw new Error('TEAM_HOST_PROTOCOL');
        requestSequence = sequence;
        if (requests.size >= 8 || (value.type === 'permission' && value.tool_name !== 'Bash')) throw new Error('TEAM_HOST_PROTOCOL');
        // Authorization polling must keep receiving tokens while an owner reads
        // a permission dialog. Framing is serial; the bounded callbacks are not.
        const request = (async () => {
          if (value.type === 'credential') {
            const token = await options.token(); options.assertCurrent();
            send({ version: 1, type: 'credential', id: value.id, owner_uid: options.bootstrap.owner_uid,
              account_epoch: options.bootstrap.account_epoch, id_token: token });
          } else {
            const allow = await options.permission({ target: options.bootstrap.target, toolName: 'Bash', input: value.input }, controller.signal).catch(() => false);
            options.assertCurrent(); send({ version: 1, type: 'permission', id: value.id, allow: allow === true });
          }
        })();
        requests.add(request);
        void request.catch(() => finish(new Error('TEAM_HOST_PROTOCOL'))).finally(() => requests.delete(request));
      } else if (value.type === 'result') {
        exact(value, ['version', 'type', 'status'], ['state', 'reply_id']);
        if (!['settled', 'busy', 'recovery_required', 'unstarted'].includes(value.status as string)
          || (value.status === 'unstarted' && (options.bootstrap.mode !== 'retry_result' || value.state !== undefined || value.reply_id !== undefined))
          || (value.state !== undefined && (typeof value.state !== 'string' || !/^[a-z_]{1,40}$/u.test(value.state)))
          || (value.reply_id !== undefined && !identifier(value.reply_id))) throw new Error('TEAM_HOST_PROTOCOL');
        result = { status: value.status as TeamHostResult['status'], ...(value.state !== undefined ? { state: value.state as string } : {}),
          ...(value.reply_id !== undefined ? { reply_id: value.reply_id as string } : {}) };
      } else if (value.type === 'error') {
        exact(value, ['version', 'type', 'code']);
        if (typeof value.code !== 'string' || !/^(?:ACCOUNT_CHANGED|MODEL_UNAVAILABLE|HTTP_[1-5][0-9]{2}|BUSY|CANCELLED|SERVICE_UNAVAILABLE|PROTOCOL|STORAGE|CONFIGURATION|RESULT_PENDING)$/u.test(value.code)) throw new Error('TEAM_HOST_PROTOCOL');
        throw new TeamHostFailure(value.code);
      } else throw new Error('TEAM_HOST_PROTOCOL');
    };
    child.stdout.on('data', (data: Buffer) => {
      bytes += data.length;
      if (bytes > 1024 * 1024) { finish(new Error('TEAM_HOST_PROTOCOL')); return; }
      buffered = Buffer.concat([buffered, data]);
      for (;;) {
        const newline = buffered.indexOf(10);
        if (newline < 0) break;
        const line = buffered.subarray(0, newline); buffered = buffered.subarray(newline + 1);
        if (!line.length || line.length > 64 * 1024) { finish(new Error('TEAM_HOST_PROTOCOL')); return; }
        chain = chain.then(() => frame(line));
        void chain.catch(error => finish(error instanceof TeamHostFailure ? error : new Error('TEAM_HOST_PROTOCOL')));
      }
      if (buffered.length > 64 * 1024) finish(new Error('TEAM_HOST_PROTOCOL'));
    });
    // Consume without displaying provider/tool/credential material.
    child.stderr.on('data', (data: Buffer) => { bytes += data.length; if (bytes > 1024 * 1024) finish(new Error('TEAM_HOST_PROTOCOL')); });
    child.once('error', () => finish(new Error('TEAM_HOST_RECOVERY_REQUIRED')));
    child.stdin.on('error', () => finish(new Error('TEAM_HOST_RECOVERY_REQUIRED')));
    child.once('close', code => { void chain.then(() => {
      if (buffered.length || code !== 0) finish(new Error('TEAM_HOST_RECOVERY_REQUIRED'));
      else finish();
    }, () => finish(new Error('TEAM_HOST_PROTOCOL'))); });
    options.signal.addEventListener('abort', abort, { once: true });
    try { send(JSON.parse(bootstrap)); } catch { finish(new Error('INVALID_SESSION')); }
  });
}
