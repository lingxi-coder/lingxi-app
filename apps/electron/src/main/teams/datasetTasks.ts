import { createHash, randomUUID } from 'node:crypto';
import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { DatasetRunSession, type DatasetApproval, type DatasetExecutor } from '../../../../../packages/teams/dataset-run.js';
import { DATA_PROTOCOL_VERSION } from '../../../../../packages/teams/dataset-resource.js';
import type { DatasetTaskVault } from '../credential-broker.js';
import type { Account } from './service.js';
import type { CronJobDto } from '../../../../../packages/bridge-client/src/protocol.js';

export interface DatasetTaskProgram {
  executor: DatasetExecutor;
  task: string;
  recipe: unknown;
}
export interface DatasetTaskHooks {
  /** `checkDataset` is shown the approved dataset before the run opens, and throws to stop it. */
  open(program: DatasetTaskProgram, logicalRun: string, signal?: AbortSignal, privatePreview?: boolean,
    checkDataset?: (dataset: string) => void): Promise<DatasetRunSession | undefined>;
}
interface StoredApproval extends DatasetApproval {
  uid: string;
  dataset: string;
  recipeSha256: string;
}
type ProductCommand = <T>(uid: string, request: Record<string, unknown>) => Promise<T>;

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value === null || typeof value !== 'object') return value;
  const record = value as Record<string, unknown>;
  return Object.fromEntries(Object.keys(record).sort().map(key => [key, canonical(record[key])]));
}

export function taskRecipeDigest(program: DatasetTaskProgram): string {
  return createHash('sha256').update(JSON.stringify(canonical(program))).digest('hex');
}

export function teamBotDatasetKey(scope: { team: string; bot: string; conversation: string }): string {
  return 'bot-' + createHash('sha256').update(JSON.stringify([scope.team, scope.bot, scope.conversation])).digest('base64url');
}

export function teamBotDatasetProgram(scope: { team: string; bot: string; conversation: string }, revision: number,
  snapshot: string, deployment: string): DatasetTaskProgram {
  return { executor: 'teamBot', task: teamBotDatasetKey(scope), recipe: { ...scope, revision, snapshot, deployment } };
}

export function scheduledDatasetProgram(task: CronJobDto, projectPath: string): DatasetTaskProgram {
  const a = task.automation;
  return { executor: 'nativeSchedule', task: task.id, recipe: { projectPath, cron: task.cron, prompt: task.prompt,
    recurring: task.recurring, durable: task.durable, expiresAt: task.expires_at, session: task.session_id,
    model: a?.model, reasoning: a?.reasoning, mode: a?.runMode, sessionMode: a?.sessionMode, scope: a?.sessionScopeKey,
    target: a?.targetSessionId, targetScope: a?.targetSessionScopeKey } };
}
/** A local marker selects a Keychain slot. The approval and its reusable credential stay in the signed host. */
export class DatasetTaskHost implements DatasetTaskHooks {
  private readonly mutations = new Map<string, Promise<void>>();
  private async serialize<T>(key: string, f: () => Promise<T>): Promise<T> {
    const prior = this.mutations.get(key) ?? Promise.resolve();
    const work = prior.then(f);
    const settled = work.then(() => undefined, () => undefined);
    this.mutations.set(key, settled);
    try { return await work; }
    finally { if (this.mutations.get(key) === settled) this.mutations.delete(key); }
  }
  constructor(private readonly directory: string, private readonly origin: string,
    private readonly account: Account, private readonly vault: DatasetTaskVault,
    private readonly command: ProductCommand, private readonly fetcher: typeof fetch = fetch) {}

  private key(uid: string, program: Pick<DatasetTaskProgram, 'executor' | 'task'>): string {
    return 'dsv2.' + createHash('sha256').update(JSON.stringify([this.origin, uid, program.executor, program.task])).digest('hex').slice(0, 58);
  }

  /** Called after the trusted user interface confirms this program and its complete Data plan. */
  async approve(program: DatasetTaskProgram, dataset: string, plan: Record<string, unknown>, operation: string, expectedUid: string): Promise<{ permit: string; planDigest: string }> {
    const uid = this.account.snapshot().user?.uid;
    if (!uid || uid !== expectedUid) throw new Error('STALE_ACCOUNT');
    return this.serialize(this.key(uid, program), async () => {
      const recipeSha256 = taskRecipeDigest(program);
      const approval = await this.command<DatasetApproval>(uid, { protocol: DATA_PROTOCOL_VERSION, dataset, operation,
        command: { action: 'approveTask', plan: { ...plan, task: program.task, executor: program.executor, recipeSha256 } } });
      if (this.account.snapshot().user?.uid !== uid) throw new Error('STALE_ACCOUNT');
      const key = this.key(uid, program);
      await mkdir(this.directory, { recursive: true, mode: 0o700 });
      const marker = join(this.directory, key);
      const pending = marker + '.' + randomUUID();
      try {
        await writeFile(pending, JSON.stringify({ permit: approval.permit }), { mode: 0o600, flag: 'wx' });
        await this.vault.write(key, JSON.stringify({ ...approval, uid, dataset, recipeSha256 } satisfies StoredApproval));
        await rename(pending, marker);
      } finally { await rm(pending, { force: true }); }
      return { permit: approval.permit, planDigest: approval.planDigest };
    });
  }

  async open(program: DatasetTaskProgram, logicalRun: string, signal?: AbortSignal, privatePreview?: boolean,
    checkDataset?: (dataset: string) => void): Promise<DatasetRunSession | undefined> {
    const uid = this.account.snapshot().user?.uid;
    if (!uid) return undefined;
    const key = this.key(uid, program);
    let marker: { permit: string };
    try { marker = JSON.parse(await readFile(join(this.directory, key), 'utf8')); }
    catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; }
    if (privatePreview) throw new Error('A private source preview requires a separate owner-approved publication');
    const stored = await this.vault.read(key);
    if (!stored || this.account.snapshot().user?.uid !== uid) throw new Error('Dataset task approval is unavailable');
    const approval: StoredApproval = JSON.parse(stored);
    const recipeSha256 = taskRecipeDigest(program);
    if (approval.uid !== uid || approval.permit !== marker.permit || approval.recipeSha256 !== recipeSha256) {
      throw new Error('Dataset task program changed and requires approval');
    }
    checkDataset?.(approval.dataset);
    const epoch = this.account.sessionEpoch?.();
    const scopedFetch: typeof fetch = async (input, init) => {
      const current = () => this.account.snapshot().user?.uid === uid && this.account.sessionEpoch?.() === epoch;
      if (!current()) throw new Error('STALE_ACCOUNT');
      const result = await this.fetcher(input, init);
      if (!current()) throw new Error('STALE_ACCOUNT');
      return result;
    };
    return DatasetRunSession.open(this.origin, approval, { executor: program.executor, task: program.task, logicalRun, recipeSha256 },
      { fetch: scopedFetch, signal });
  }

  async revoke(program: Pick<DatasetTaskProgram, 'executor' | 'task'>, expectedUid: string): Promise<void> {
    const uid = this.account.snapshot().user?.uid;
    if (!uid || uid !== expectedUid) throw new Error('STALE_ACCOUNT');
    const key = this.key(uid, program);
    await this.serialize(key, async () => {
      let marker: { permit: string };
      try { marker = JSON.parse(await readFile(join(this.directory, key), 'utf8')); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return; throw error; }
      const stored = await this.vault.read(key);
      if (!stored) throw new Error('Dataset task approval is unavailable');
      const approval: StoredApproval = JSON.parse(stored);
      if (approval.uid !== uid || approval.permit !== marker.permit || this.account.snapshot().user?.uid !== uid) throw new Error('STALE_ACCOUNT');
      await this.command(uid, { protocol: DATA_PROTOCOL_VERSION, dataset: approval.dataset, command: { action: 'revokeTask', permit: approval.permit } });
      await this.vault.remove(key);
      await rm(join(this.directory, key), { force: true });
    });
  }
}
