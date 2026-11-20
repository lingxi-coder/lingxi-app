import { execFile } from 'node:child_process';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { promisify } from 'node:util';

const invoke = promisify(execFile);

export interface PrivatePreviewProof {
  teamId: string;
  generation: number;
  taskId: string;
  deploymentId: string;
  hostId: string;
  accountEpoch: number;
  permitId: string;
  expectedLastChangeSeq: number;
  source: string;
}

export interface PrivatePreviewLease {
  token: string;
  expires: number;
}

export interface PrivatePreviewPage {
  rows: Record<string, unknown>[];
  cursor: string | null;
  truncated: boolean;
}

export function validPreviewLease(lease: PrivatePreviewLease): boolean {
  return typeof lease?.token === 'string' && /^[A-Za-z0-9_-]{43}$/u.test(lease.token)
    && Number.isSafeInteger(lease.expires) && lease.expires > Date.now();
}

export async function readPrivateDataPage(source: string, lease: PrivatePreviewLease | undefined, identity: string,
  directory: string, signal: AbortSignal): Promise<PrivatePreviewPage> {
  const bin = process.env.LINGXI_DATA_PREVIEW_BIN;
  const ca = process.env.LINGXI_DATA_PREVIEW_CA;
  const ice = process.env.LINGXI_DATA_PREVIEW_ICE;
  const origin = process.env.LINGXI_DATA_PREVIEW_ORIGIN;
  if (!bin || !isAbsolute(bin) || !origin || (ca && !isAbsolute(ca)) || (ice && !isAbsolute(ice))) throw new Error('DATA_PREVIEW_UNAVAILABLE');
  const url = new URL(origin);
  if (url.protocol !== 'https:' || url.username || url.password || url.pathname !== '/' || url.search || url.hash
    || (lease !== undefined && !validPreviewLease(lease)) || !/^[A-Za-z0-9_-]{1,64}$/u.test(source)) throw new Error('DATA_PREVIEW_UNAVAILABLE');
  signal.throwIfAborted();
  const attempt = await mkdtemp(join(directory, 'preview-'));
  try {
    const authPath = join(attempt, 'identity'), taskPath = join(attempt, 'permit');
    await writeFile(authPath, identity, { mode: 0o600, flag: 'wx' });
    if (lease) await writeFile(taskPath, lease.token, { mode: 0o600, flag: 'wx' });
    const { stdout } = await invoke(bin, ['preview', '--endpoint', url.origin, '--auth-file', authPath,
      ...(lease ? ['--task-token-file', taskPath] : []), '--source', source, ...(ca ? ['--ca', ca] : []), ...(ice ? ['--ice', ice] : [])],
    { cwd: attempt, signal, timeout: lease ? Math.min(45_000, Math.max(1, lease.expires - Date.now())) : 45_000, maxBuffer: 1024 * 1024,
      env: { HOME: attempt, TMPDIR: attempt, PATH: '/usr/bin:/bin' } });
    const page = JSON.parse(stdout) as PrivatePreviewPage;
    if (!Array.isArray(page?.rows) || page.rows.some(row => !row || typeof row !== 'object' || Array.isArray(row))
      || (page.cursor !== null && typeof page.cursor !== 'string') || typeof page.truncated !== 'boolean') throw new Error('Invalid source page');
    return page;
  } catch {
    throw new Error('DATA_PREVIEW_UNAVAILABLE');
  } finally { await rm(attempt, { recursive: true, force: true }); }
}
