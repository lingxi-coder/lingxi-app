import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { parseDecisionConfiguration, type DecisionCommand, type DecisionConfiguration, type DecisionInput, type DecisionResult, type DecisionSnapshot } from '../shared/decision.js';
import type { DecisionCredentials } from './decisionCredentials.js';
import type { ProviderAccountLaunch } from './provider-accounts.js';
import { resolveServerBin } from './bridgeDiscovery.js';
import { buildBridgeEnvironment } from './host-utils.js';

type Stored = { revision: string; configuration: DecisionConfiguration | null; credentialRef?: string };
export class DecisionHost {
  private readonly path: string;
  private busy = false;
  private readonly configurationListeners = new Set<() => void>();
  private readonly operations = new Map<string, { owner: number | string; abort: AbortController }>();
  constructor(private readonly options: { userData: string; isPackaged: boolean; resourcesPath: string; broker?: DecisionCredentials;
    accounts(accountId: string): Promise<ProviderAccountLaunch | undefined>; changed(snapshot: DecisionSnapshot): void;
    execute?: (envelope: unknown, signal: AbortSignal) => Promise<DecisionResult> }) {
    this.path = join(options.userData, 'decision.json');
  }
  private read(): Stored {
    try {
      const bytes = readFileSync(this.path);
      if (bytes.length > 65536) throw new Error('Decision settings are too large');
      const value = JSON.parse(bytes.toString('utf8')) as Stored;
      if (typeof value.revision !== 'string' || !value.revision || value.credentialRef !== undefined && !/^decision\.[a-f0-9]{48}$/.test(value.credentialRef)) throw new Error('Invalid Decision settings');
      return { revision: value.revision, configuration: parseDecisionConfiguration(value.configuration), ...(value.credentialRef ? { credentialRef: value.credentialRef } : {}) };
    } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { revision: 'initial', configuration: null }; throw error; }
  }
  async snapshot(): Promise<DecisionSnapshot> {
    const value = this.read();
    const credentialConfigured = value.credentialRef ? (await this.options.broker?.preview(value.credentialRef))?.configured === true : false;
    return { revision: value.revision, configuration: value.configuration, credentialConfigured };
  }
  async isConfigured(): Promise<boolean> {
    const current = this.read();
    const config = current.configuration;
    // Discovery observes saved configuration, never a broker/network health probe.
    // Execution resolves the actual credential and reports any unavailable key.
    return !!config && (config.source === 'custom' || !!config.accountId || !!current.credentialRef);
  }
  onConfigurationChanged(listener: () => void): () => void {
    this.configurationListeners.add(listener);
    return () => this.configurationListeners.delete(listener);
  }
  evaluateTool(input: DecisionInput, signal: AbortSignal, timeoutMs: number): Promise<DecisionResult> {
    return this.evaluate('tool', randomUUID(), input, signal, timeoutMs);
  }
  async command(owner: number, raw: unknown, assertCurrent: () => void = () => undefined): Promise<DecisionSnapshot | DecisionResult | void> {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('Invalid Decision command');
    const command = raw as DecisionCommand;
    switch (command.action) {
      case 'get': return this.snapshot();
      case 'save': return this.save(command, assertCurrent);
      case 'cancel': {
        const operation = this.operations.get(command.operationId);
        if (operation?.owner === owner) operation.abort.abort();
        return;
      }
      case 'evaluate': return this.evaluate(owner, command.operationId, command.input);
      default: throw new Error('Unknown Decision command');
    }
  }
  private async save(command: Extract<DecisionCommand, { action: 'save' }>, assertCurrent: () => void): Promise<DecisionSnapshot> {
    if (this.busy) throw new Error('Decision settings are being saved');
    this.busy = true;
    let newCredentialRef: string | undefined;
    let committed = false;
    try {
      const current = this.read();
      if (command.revision !== current.revision) throw new Error('Decision settings changed in another window. Reload before saving.');
      const configuration = parseDecisionConfiguration(command.configuration);
      const sameEndpoint = configuration?.endpoint === current.configuration?.endpoint && configuration?.protocol === current.configuration?.protocol;
      let credentialRef = sameEndpoint && !configuration?.accountId ? current.credentialRef : undefined;
      if (command.credential !== undefined && command.credential !== '') {
        if (configuration?.accountId) throw new Error('Select either a saved account or a separate API key');
        if (typeof command.credential !== 'string' || /[\r\n\0]/.test(command.credential) || command.credential.length > 16384 || !configuration) throw new Error('Invalid API key');
        if (!this.options.broker) throw new Error('Secure credential storage is unavailable');
        // New immutable reference makes a failed settings write leave the old binding intact.
        credentialRef = `decision.${createHash('sha256').update(randomUUID()).digest('hex').slice(0, 48)}`;
        newCredentialRef = credentialRef;
        await this.options.broker.set(credentialRef, command.credential);
      } else if (command.credential === '') credentialRef = undefined;
      if (this.read().revision !== current.revision) throw new Error('Decision settings changed while saving. Reload before saving.');
      assertCurrent();
      const next: Stored = { revision: randomUUID(), configuration, ...(configuration && credentialRef ? { credentialRef } : {}) };
      mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
      const temporary = `${this.path}.${randomUUID()}.tmp`;
      writeFileSync(temporary, JSON.stringify(next), { mode: 0o600, flag: 'wx' });
      renameSync(temporary, this.path);
      committed = true;
      for (const listener of this.configurationListeners) listener();
      if (current.credentialRef && current.credentialRef !== next.credentialRef) await this.options.broker?.delete(current.credentialRef).catch(() => undefined);
      const snapshot = await this.snapshot();
      this.options.changed(snapshot);
      return snapshot;
    } finally {
      if (!committed && newCredentialRef) await this.options.broker?.delete(newCredentialRef).catch(() => undefined);
      this.busy = false;
    }
  }
  private async evaluate(owner: number | string, operationId: string, input: DecisionInput, signal?: AbortSignal, timeoutMs = 30_000): Promise<DecisionResult> {
    if (signal?.aborted) throw new Error('Decision operation cancelled');
    if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 30_000) throw new Error('Invalid Decision timeout');
    if (typeof operationId !== 'string' || !/^[a-zA-Z0-9_-]{1,128}$/.test(operationId) || this.operations.has(operationId)) throw new Error('Invalid Decision operation ID');
    if (this.operations.size >= 8) throw new Error('Decision service is busy');
    if (!input || typeof input !== 'object' || !input.questions || typeof input.questions !== 'object' || Array.isArray(input.questions)) throw new Error('Invalid Decision input');
    const current = this.read(); const config = current.configuration;
    if (!config) throw new Error('Choose and save a Decision service first');
    const abort = new AbortController(); this.operations.set(operationId, { owner, abort });
    const cancel = () => abort.abort();
    signal?.addEventListener('abort', cancel, { once: true });
    const timer = setTimeout(cancel, timeoutMs);
    let onAbort!: () => void;
    const cancelled = new Promise<never>((_resolve, reject) => {
      onAbort = () => reject(new Error('Decision operation cancelled or timed out'));
      abort.signal.addEventListener('abort', onAbort, { once: true });
    });
    const perform = async () => {
      let credential: string | undefined;
      if (config.accountId) {
        const launch = await this.options.accounts(config.accountId);
        const account = launch?.providerAccounts.accounts.find(a => a.accountId === config.accountId && a.enabled && a.authKind === 'api_key' && a.executionBindingId === 'openai' && a.providerId === (config.source === 'openai' ? 'openai' : 'typesafe'));
        const grant = account && launch?.providerAccountCredentials[account.accountId];
        if (!account || !grant || grant.identity_generation !== account.identityGeneration || grant.kind.kind !== 'api_key') throw new Error('The selected Decision API account is unavailable');
        credential = grant.kind.key;
      } else if (current.credentialRef) {
        credential = await this.options.broker?.resolve(current.credentialRef);
        if (!credential) throw new Error('Decision API key is unavailable');
      }
      if (config.source !== 'custom' && !credential) throw new Error('Configure an API key for this Decision service');
      if (abort.signal.aborted) throw new Error('Decision operation cancelled or timed out');
      const envelope = { endpoint: config.endpoint, protocol: config.protocol, credential: credential ?? null,
        request: { model: { backend: config.protocol, account: config.accountId ?? 'endpoint', model: config.model }, state_revision: 0, state: input.state, images: input.images ?? [], questions: input.questions } };
      const result = await (this.options.execute ?? ((value, signal) => this.run(value, signal)))(envelope, abort.signal);
      if (abort.signal.aborted) throw new Error('Decision operation cancelled or timed out');
      return result;
    };
    const running = perform();
    // Cancellation returns promptly, but non-abortable credential work still owns its slot.
    void running.then(() => this.operations.delete(operationId), () => this.operations.delete(operationId));
    try { return await Promise.race([running, cancelled]); }
    finally { clearTimeout(timer); signal?.removeEventListener('abort', cancel); abort.signal.removeEventListener('abort', onAbort); }
  }
  private run(envelope: unknown, signal: AbortSignal): Promise<DecisionResult> {
    const input = JSON.stringify(envelope);
    if (Buffer.byteLength(input) > 16 * 1024 * 1024) return Promise.reject(new Error('Decision input exceeds 16 MiB'));
    return new Promise((resolve, reject) => {
      const child = spawn(resolveServerBin(this.options), ['--decision-service-json'], { env: buildBridgeEnvironment(process.env), stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
      const chunks: Buffer[] = []; let size = 0; let settled = false;
      const finish = (error?: Error, value?: DecisionResult) => { if (settled) return; settled = true; signal.removeEventListener('abort', cancel); if (error) { child.kill(); reject(error); } else resolve(value!); };
      const cancel = () => finish(new Error('Decision operation cancelled or timed out'));
      signal.addEventListener('abort', cancel, { once: true }); if (signal.aborted) cancel();
      child.stdout.on('data', (chunk: Buffer) => { size += chunk.length; if (size > 16 * 1024 * 1024) finish(new Error('Decision response exceeds 16 MiB')); else chunks.push(chunk); });
      child.stderr.on('data', () => undefined);
      child.once('error', error => finish(error)); child.stdin.once('error', error => finish(error));
      child.once('close', code => {
        if (settled) return;
        if (code !== 0) return finish(new Error('Decision service could not complete the operation'));
        try { const response = JSON.parse(Buffer.concat(chunks).toString('utf8')); if (response.error) finish(new Error(String(response.error))); else if (response.result?.answers && response.result?.usage) finish(undefined, response.result); else finish(new Error('Invalid Decision response')); }
        catch { finish(new Error('Invalid Decision response')); }
      });
      child.stdin.end(input);
    });
  }
  cancelOwner(owner: number): void { for (const op of this.operations.values()) if (op.owner === owner) op.abort.abort(); }
  dispose(): void { this.configurationListeners.clear(); for (const op of this.operations.values()) op.abort.abort(); }
}
