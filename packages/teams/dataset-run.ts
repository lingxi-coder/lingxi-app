import { isResourceRef, type ResourceRef } from './dataset-resource.js';

export type DatasetExecutor = 'nativeSchedule' | 'teamBot' | 'localAppAgent';
interface DatasetBindingDetails {
  dataset: string;
  operation: string;
  plan: Record<string, unknown>;
}
export interface DatasetBotScope { team: string; bot: string; conversation: string }
export type DatasetBindingDraft = DatasetBindingDetails & (
  | { executor: 'nativeSchedule'; task: string; scope: string }
  | ({ executor: 'teamBot' } & DatasetBotScope)
  | { executor: 'localAppAgent'; task: string; recipe: unknown }
);
export interface DatasetApproval {
  permit: string;
  planDigest: string;
  credential: string;
}
export interface DatasetExecution {
  executor: DatasetExecutor;
  task: string;
  logicalRun: string;
  recipeSha256: string;
}
type RunCommand = { action: 'start'; executor: DatasetExecutor; task: string; recipeSha256: string }
  | { action: 'effect'; step: string } | { action: 'readInput'; index: number; block?: number; after?: string }
  | { action: 'put'; step: string; expectedRevision: number; value: Record<string, unknown> }
  | { action: 'publish'; step: string } | { action: 'finish' };

interface OutputBinding {
  step: string;
  expectedRevision: number;
  fields: Record<string, string[]>;
}
function isOutputBinding(v: unknown): v is OutputBinding {
  return object(v) && typeof v.step === 'string' && typeof v.expectedRevision === 'number' && Number.isSafeInteger(v.expectedRevision)
    && v.expectedRevision >= 0 && object(v.fields) && Object.values(v.fields).every(path => Array.isArray(path) && path.length > 0 &&
      path.length <= 8 && path.every(part => typeof part === 'string'));
}

export class DatasetRunError extends Error {
  constructor(readonly status: number, message: string) { super(message); }
}

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

async function waitForDatasetRun(deadline: number, message: string, signal?: AbortSignal): Promise<void> {
  signal?.throwIfAborted();
  const remaining = deadline - Date.now();
  if (remaining <= 0) throw new DatasetRunError(503, message);
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', abort);
      resolve();
    }, Math.min(500, remaining));
    function abort(): void {
      clearTimeout(timer);
      reject(signal?.reason);
    }
    signal?.addEventListener('abort', abort, { once: true });
  });
}

/** Trusted host transport. Only references and projected results may leave this boundary. */
export class DatasetRunSession {
  private constructor(private readonly send: (command: RunCommand, signal?: AbortSignal) => Promise<Record<string, unknown>>,
    readonly inputs: readonly ResourceRef[], readonly automaticSteps: readonly string[], private readonly bindings: readonly OutputBinding[], readonly finished: boolean) {}

  static async open(origin: string, approval: DatasetApproval, execution: DatasetExecution,
    options: { fetch?: typeof fetch; signal?: AbortSignal } = {}): Promise<DatasetRunSession> {
    const endpoint = new URL(origin);
    if (endpoint.protocol !== 'https:' || endpoint.username || endpoint.password || endpoint.pathname !== '/' || endpoint.search || endpoint.hash) {
      throw new Error('Invalid Dataset service origin');
    }
    endpoint.pathname = '/v2/dataset-runs';
    const fetcher = options.fetch ?? fetch;
    let runCredential: string | undefined;
    let expires = 0;
    let refreshing: Promise<void> | undefined;
    async function request(command: RunCommand, signal?: AbortSignal): Promise<Record<string, unknown>> {
      const opening = command.action === 'start';
      const credential = opening ? approval.credential : runCredential;
      if (!credential) throw new DatasetRunError(403, 'Run capability unavailable');
      const signalForRequest = signal ?? options.signal;
      const timeout = AbortSignal.timeout(30_000);
      const response = await fetcher(endpoint, { method: 'POST', redirect: 'error', cache: 'no-store',
        signal: signalForRequest ? AbortSignal.any([signalForRequest, timeout]) : timeout,
        headers: { Authorization: (opening ? 'DatasetTask ' : 'DatasetRun ') + credential, 'Content-Type': 'application/json' },
        body: JSON.stringify({ logicalRun: execution.logicalRun, planDigest: approval.planDigest, command }) });
      const value: unknown = await response.json();
      if (!response.ok) throw new DatasetRunError(response.status, object(value) && typeof value.error === 'string' ? value.error : 'Dataset request failed');
      if (!object(value)) throw new DatasetRunError(502, 'Invalid Dataset response');
      return value;
    }
    const start = { action: 'start' as const, executor: execution.executor, task: execution.task, recipeSha256: execution.recipeSha256 };
    async function refresh(signal?: AbortSignal): Promise<void> {
      const renewed = await request(start, signal);
      if (typeof renewed.credential !== 'string' || typeof renewed.expires !== 'number') throw new DatasetRunError(403, 'Run capability unavailable');
      runCredential = renewed.credential;
      expires = renewed.expires;
    }
    async function send(command: RunCommand, signal?: AbortSignal): Promise<Record<string, unknown>> {
      if (expires <= Date.now() / 1000 + 15) {
        refreshing ??= refresh(signal).finally(() => { refreshing = undefined; });
        await refreshing;
      }
      return request(command, signal);
    }
    const deadline = Date.now() + 30_000;
    let ready = await request(start);
    while (ready.status === 'preparing') {
      await waitForDatasetRun(deadline, 'Snapshot preparation is still in progress', options.signal);
      ready = await request(start);
    }
    if (!Array.isArray(ready.inputs) || !ready.inputs.every(isResourceRef)
      || !Array.isArray(ready.automaticSteps) || !ready.automaticSteps.every(s => typeof s === 'string')
      || !Array.isArray(ready.outputBindings) || !ready.outputBindings.every(isOutputBinding)
      || (ready.finished !== true && (typeof ready.expires !== 'number' || typeof ready.credential !== 'string'))) throw new DatasetRunError(502, 'Invalid Dataset run');
    runCredential = typeof ready.credential === 'string' ? ready.credential : undefined;
    expires = typeof ready.expires === 'number' ? ready.expires : 0;
    return new DatasetRunSession(send, ready.inputs, ready.automaticSteps, ready.outputBindings, ready.finished === true);
  }

  readInput(index: number, block?: number, signal?: AbortSignal): Promise<Record<string, unknown>> {
    return this.send({ action: 'readInput', index, block }, signal);
  }
  readReport(index: number, after?: string, signal?: AbortSignal): Promise<Record<string, unknown>> {
    return this.send({ action: 'readInput', index, after }, signal);
  }

  /** The first projected page of each input. Further pages require an explicit read. */
  async modelInputs(signal?: AbortSignal): Promise<{ resource: ResourceRef; content: string }[]> {
    if (this.finished) return [];
    const inputs: { resource: ResourceRef; content: string }[] = [];
    let remaining = 64 * 1024;
    for (const [index, resource] of this.inputs.entries()) {
      const data = await this.readInput(index, resource.kind === 'snapshot' ? 0 : undefined, signal);
      let value: unknown;
      switch (resource.kind) {
        case 'record':
          value = data.value;
          break;
        case 'snapshot':
          value = { records: Array.isArray(data.records) ? data.records.map(row => object(row) ?
            { reference: row.reference, value: row.value } : null) : [],
          more: data.nextBlock !== null && data.nextBlock !== undefined };
          break;
        case 'reportRelease':
          value = { title: data.title, metric: data.metric, groups: data.groups, more: typeof data.cursor === 'string' };
          break;
        default:
          throw new DatasetRunError(502, 'Invalid task input');
      }
      const content = JSON.stringify(value);
      if (content === undefined) throw new DatasetRunError(502, 'Invalid task input');
      remaining -= new TextEncoder().encode(content).length;
      if (remaining < 0) throw new DatasetRunError(413, 'Approved task inputs exceed the model input limit');
      inputs.push({ resource, content });
    }
    return inputs;
  }

  async put(step: string, expectedRevision: number, value: Record<string, unknown>, signal?: AbortSignal): Promise<ResourceRef> {
    const result = await this.send({ action: 'put', step, expectedRevision, value }, signal);
    if (!isResourceRef(result.reference)) throw new DatasetRunError(502, 'Invalid write receipt');
    return result.reference;
  }
  async finish(output: unknown, signal?: AbortSignal): Promise<void> {
    if (this.finished) return;
    for (const binding of this.bindings) {
      if ((await this.send({ action: 'effect', step: binding.step }, signal)).applied === true) continue;
      const projected = Object.create(null) as Record<string, unknown>;
      for (const [field, path] of Object.entries(binding.fields)) {
        let value: unknown = output;
        for (const segment of path) {
          if (!object(value) || !Object.prototype.hasOwnProperty.call(value, segment)) throw new DatasetRunError(400, 'Approved task output is missing');
          value = value[segment];
        }
        projected[field] = value;
      }
      await this.put(binding.step, binding.expectedRevision, projected, signal);
    }
    for (const step of this.automaticSteps) {
      const deadline = Date.now() + 30_000;
      let reply = await this.send({ action: 'publish', step }, signal);
      while (reply.status === 'awaitingDelivery') {
        await waitForDatasetRun(deadline, 'Publication is awaiting delivery', signal);
        reply = await this.send({ action: 'publish', step }, signal);
      }
      if (reply.status !== 'published') throw new DatasetRunError(409, 'Publication requires attention');
    }
    await this.send({ action: 'finish' }, signal);
  }
}
