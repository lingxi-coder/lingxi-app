import { spawn, type ChildProcess } from 'node:child_process';
import type { McpPrepareStatus } from '../shared/mcpPrepare.js';

/** The longest a first install may take. Saving goes ahead afterwards either way. */
export const MCP_PREPARE_DEADLINE_MS = 10 * 60_000;
/** How long the server gets to stop after SIGTERM, and again after SIGKILL. */
const STOP_GRACE_MS = 3_000;
/** Characters of one unfinished stdout line kept while waiting for the `initialize` answer. */
const MAX_LINE = 1 << 20;

const INITIALIZE = `${JSON.stringify({
  jsonrpc: '2.0',
  id: 1,
  method: 'initialize',
  params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'lingxi-desktop', version: '1' } },
})}\n`;

export interface McpPrepareLaunch {
  command: string;
  args: readonly string[];
  env: NodeJS.ProcessEnv;
  cwd: string;
  deadlineMs: number;
  stopGraceMs?: number;
}

function answersInitialize(line: string): boolean {
  try {
    const message = JSON.parse(line) as { id?: unknown; result?: unknown; error?: unknown } | null;
    return message?.id === 1 && (message.result !== undefined || message.error !== undefined);
  } catch {
    return false;
  }
}

function settledWithin(promise: Promise<void>, ms: number): Promise<boolean> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(false), ms);
    void promise.then(() => {
      clearTimeout(timer);
      resolve(true);
    });
  });
}

// Whatever the outcome, the install is over or abandoned, so nothing is left to finish gracefully.
async function stop(child: ChildProcess, closed: Promise<void>, group: boolean, graceMs: number): Promise<void> {
  child.stdin?.end();
  for (const signal of ['SIGTERM', 'SIGKILL'] as const) {
    try {
      if (group && child.pid !== undefined) process.kill(-child.pid, signal);
      else child.kill(signal);
    } catch {
      // The process group is already gone.
    }
    if (await settledWithin(closed, graceMs)) return;
  }
}

/**
 * Start a stdio MCP server the way the runtime will, send `initialize`, and stop the server once it answers,
 * exits or runs out of time. The server gets its own process group, so npx or uvx and the server under it stop
 * together.
 */
export async function prepareMcpServer(launch: McpPrepareLaunch): Promise<McpPrepareStatus> {
  const group = process.platform !== 'win32';
  let child: ChildProcess;
  try {
    child = spawn(launch.command, [...launch.args], {
      cwd: launch.cwd, env: launch.env, stdio: ['pipe', 'pipe', 'ignore'], detached: group, windowsHide: true,
    });
  } catch {
    return 'exited';
  }
  const closed = new Promise<void>((resolve) => {
    child.once('close', () => resolve());
    child.once('error', () => resolve());
  });
  const status = await new Promise<McpPrepareStatus>((resolve) => {
    const timer = setTimeout(() => resolve('timeout'), launch.deadlineMs);
    const settle = (value: McpPrepareStatus) => {
      clearTimeout(timer);
      resolve(value);
    };
    child.once('error', () => settle('exited'));
    child.once('exit', () => settle('exited'));
    let line = '';
    child.stdout!.setEncoding('utf8');
    child.stdout!.on('data', (chunk: string) => {
      line += chunk;
      for (let newline = line.indexOf('\n'); newline >= 0; newline = line.indexOf('\n')) {
        if (answersInitialize(line.slice(0, newline))) settle('ready');
        line = line.slice(newline + 1);
      }
      if (line.length > MAX_LINE) line = '';
    });
    child.stdin!.on('error', () => undefined);
    child.stdin!.write(INITIALIZE);
  });
  await stop(child, closed, group, launch.stopGraceMs ?? STOP_GRACE_MS);
  return status;
}
