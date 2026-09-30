import { spawn, type ChildProcess } from 'node:child_process';
import { statSync } from 'node:fs';
import path from 'node:path';
import { ENV_ALLOWLIST } from '../host-utils.js';
export interface CommandResult { stdout: Buffer; stderr: string; code: number; truncated: boolean }
const children = new Map<ChildProcess, string>();
// A bare program name is resolved by libuv against the CHILD's cwd before PATH
// on Windows, and every one of these spawns sets cwd to the repository root. A
// `git.exe` committed to an untrusted repo would therefore run instead of the
// real Git. So resolving through PATH ourselves is the guard — and it only holds
// if a FAILED lookup refuses to spawn. Returning the bare name as a fallback
// would hand the repository exactly the lookup this function exists to prevent.
function resolveOnPath(name: string): string | null {
  const exts = process.platform === 'win32' ? (process.env.PATHEXT ?? '.EXE;.CMD;.BAT').split(';') : [''];
  for (const dir of (process.env.PATH ?? '').split(path.delimiter)) {
    if (!dir) continue;
    for (const ext of exts) {
      const candidate = path.join(dir, name + ext);
      try { if (statSync(candidate).isFile()) return candidate; } catch { /* next candidate */ }
    }
  }
  return null;
}
// Resolved on first use and re-resolved whenever PATH changes, NOT frozen at
// module load: the Electron main process is launched by the OS with a minimal
// PATH and repairs it afterwards, so a load-time constant can capture a lookup
// that was going to fail (or miss the Git the user actually installed). It also
// keeps the resolution observable — a caller that clears PATH really does get
// "no Git", which is what makes the unavailable-Git path testable at all.
let gitBinPath: string | null = null;
let gitBinFor: string | undefined;
function gitBin(): string {
  const search = process.env.PATH ?? '';
  if (gitBinFor !== search) { gitBinFor = search; gitBinPath = resolveOnPath('git'); }
  if (gitBinPath) return gitBinPath;
  const error: NodeJS.ErrnoException = new Error('Git was not found on PATH.');
  error.code = 'ENOENT';
  throw error;
}
const TASKKILL_BIN = (process.platform === 'win32' ? resolveOnPath('taskkill') : null) ?? 'taskkill';
// Git runs repository-controlled code — `pre-commit`/`post-checkout`/`pre-push`
// hooks and whatever `credential.helper` names — so its child gets the same
// allowlist treatment as the bridge (`buildBridgeEnvironment`) and the terminal
// (`terminalEnvironment`), not the host's whole environment. A denylist of eight
// `GIT_*` keys would have passed every model credential and `NODE_OPTIONS` the
// desktop was launched with straight into a hook.
const GIT_ENV_ALLOWLIST: readonly string[] = [
  ...ENV_ALLOWLIST,
  // Auth and signing paths Git legitimately needs and that carry no secret.
  'SSH_AUTH_SOCK', 'SSH_ASKPASS', 'GNUPGHOME', 'GPG_TTY',
  'HOMEDRIVE', 'HOMEPATH', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME',
];
function terminate(child: ChildProcess, force = false): void {
  if (!child.pid) return;
  if (process.platform === 'win32') {
    const killer = spawn(TASKKILL_BIN, ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
    killer.on('error', () => child.kill());
  } else {
    try { process.kill(-child.pid, force ? 'SIGKILL' : 'SIGTERM'); } catch { try { child.kill(force ? 'SIGKILL' : 'SIGTERM'); } catch {} }
  }
}
export function stopGitCommands(roots: Set<string>): void { for (const [child, cwd] of children) if (roots.has(cwd)) terminate(child, true); }
export function git(cwd: string, args: string[], input?: string | Buffer, allowFailure = false, maxBytes?: number): Promise<CommandResult> {
  const limit = maxBytes ?? 8 * 1024 * 1024;
  return new Promise((resolve, reject) => {
    const env: NodeJS.ProcessEnv = {};
    for (const key of GIT_ENV_ALLOWLIST) { const value = process.env[key]; if (value !== undefined) env[key] = value; }
    // The allowlist already excludes every `GIT_*` key, so the inherited
    // repository-pointing overrides this used to delete cannot be present.
    Object.assign(env, { GIT_OPTIONAL_LOCKS: '0', GIT_TERMINAL_PROMPT: '0', GCM_INTERACTIVE: 'Never', GIT_EDITOR: 'true', GIT_SEQUENCE_EDITOR: 'true', LC_ALL: 'C.UTF-8' });
    let bin: string;
    try { bin = gitBin(); } catch (error) { reject(error as Error); return; }
    const child = spawn(bin, ['--literal-pathspecs', ...args], { cwd, detached: process.platform !== 'win32', windowsHide: true, shell: false, env, stdio: ['pipe', 'pipe', 'pipe'] });
    children.set(child, cwd);
    const chunks: Buffer[] = []; let size = 0; let stderr = ''; let truncated = false; let timedOut = false;
    const timer = setTimeout(() => { timedOut = true; terminate(child); setTimeout(() => terminate(child, true), 1500).unref(); }, 120000); timer.unref();
    child.stdout.on('data', (data: Buffer) => { const remaining = limit - size; if (remaining > 0) { chunks.push(data.subarray(0, remaining)); size += Math.min(remaining, data.length); } if (data.length > remaining) { truncated = true; if (maxBytes === undefined) { terminate(child); setTimeout(() => terminate(child, true), 1500).unref(); } } });
    child.stderr.on('data', data => { stderr = (stderr + data.toString()).slice(-65536); });
    child.on('error', error => { children.delete(child); clearTimeout(timer); reject(error); });
    child.on('close', code => {
      children.delete(child); clearTimeout(timer);
      if (timedOut) { reject(new Error('Git timed out. Complete interactive authentication or signing in the terminal, then retry.')); return; }
      if (truncated && maxBytes === undefined) { reject(new Error('Git result exceeds the safe size limit; use the terminal for this repository operation.')); return; }
      const result = { stdout: Buffer.concat(chunks), stderr, code: code ?? -1, truncated };
      if (code !== 0 && !allowFailure) reject(new Error(`${stderr.trim() || `Git exited with code ${code}`}${/auth|credential|passphrase|signing|terminal prompts/i.test(stderr) ? '\nComplete authentication or signing in the terminal, then retry.' : ''}`)); else resolve(result);
    });
    child.stdin.on('error', () => {}); child.stdin.end(input);
  });
}
