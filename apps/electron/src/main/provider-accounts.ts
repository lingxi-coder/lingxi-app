import { providerById, type ProviderDefinition } from '../shared/providers.js';
import { randomUUID } from 'node:crypto';
import { link, lstat, mkdir, open, readFile, rm, unlink } from 'node:fs/promises';
import { join } from 'node:path';
import type { ProviderAccountCommand, ProviderAccountMetadata, ProviderAccountSnapshot } from '../shared/providerAccounts.js';
import { logicalProviderId, type ProviderAccountRouting } from '../shared/providerAccounts.js';
import type { ProviderCredentialBroker } from './credential-broker.js';
import { CODEX_PROVIDER_ID, type CodexOAuthSession, parseCodexSession } from './codex-auth.js';
import { codexAccessToken, type CodexSessionStore, type CodexTokenManager } from './codex-tokens.js';

export type ProviderAccountRenewFailure = 'account_missing' | 'identity_changed' | 'storage_pending' | 'credential_missing' | 'api_key_rejected' | 'revision_changed' | 'snapshot_failed' | 'lock_failed' | 'pending_check_failed' | 'broker_read_failed' | 'decode_failed' | 'token_refresh_failed' | 'authority_unavailable';

export class ProviderAccountRenewError extends Error {
  constructor(readonly code: Exclude<ProviderAccountRenewFailure, 'authority_unavailable'>, message: string) {
    super(message);
  }
}

/** Never expose an arbitrary provider, broker or credential error to diagnostics. */
export function providerAccountRenewFailureCode(error: unknown): ProviderAccountRenewFailure {
  return error instanceof ProviderAccountRenewError ? error.code : 'authority_unavailable';
}

export interface ProviderAccountCredentialVersion {
  accountId: string;
  identityGeneration: number;
  credentialRevision: number;
}

export interface ProviderAccountCredential {
  credential_revision: number;
  identity_generation: number;
  kind: { kind: 'api_key'; key: string } | ({ kind: 'oauth'; issuer: string; scopes: string[]; subject?: string } & CodexOAuthSession);
}

function parseStoredCredential(value: string): ProviderAccountCredential {
  try {
    const record = JSON.parse(value) as ProviderAccountCredential;
    if (!Number.isSafeInteger(record.credential_revision) || record.credential_revision < 1
      || !Number.isSafeInteger(record.identity_generation) || record.identity_generation < 1 || !record.kind) throw new Error();
    if (record.kind.kind === 'oauth') parseCodexSession(record.kind);
    else if (record.kind.kind !== 'api_key' || typeof record.kind.key !== 'string' || !record.kind.key || record.kind.key.length > 16_384 || record.kind.key.includes('\0')) throw new Error();
    return record;
  } catch { throw new Error('Stored credentials are unreadable. Remove this account and add it again.'); }
}

/** Exact Rust AccountCredential wire shape; OAuth grants here are access-only. */
export type RuntimeProviderAccountCredential = {
  credential_revision: number;
  identity_generation: number;
  kind: { kind: 'api_key'; key: string } | {
    kind: 'oauth'; access_token: string; refresh_token: null;
    expires_at: { secs_since_epoch: number; nanos_since_epoch: number };
    scopes: string[]; issuer: string; subject: string | null; organization_id: string | null; email: null; fedramp: boolean;
  };
};
export interface ProviderAccountLaunch {
  providerAccounts: ProviderAccountSnapshot;
  providerAccountCredentials: Record<string, RuntimeProviderAccountCredential>;
  providerAccountHome?: string;
}
export interface ProviderAccountEnvelope {
  provider_accounts: ProviderAccountSnapshot;
  provider_account_credentials: Record<string, RuntimeProviderAccountCredential>;
  provider_account_home?: string;
  provider_account_profiles: readonly Record<string, unknown>[];
}
export function providerAccountEnvelope(launch: ProviderAccountLaunch, profiles: readonly Record<string, unknown>[]): ProviderAccountEnvelope {
  return { provider_account_profiles: profiles, provider_accounts: launch.providerAccounts, provider_account_credentials: launch.providerAccountCredentials,
    ...(launch.providerAccountHome ? { provider_account_home: launch.providerAccountHome } : {}) };
}
export function runtimeAccountCredential(stored: ProviderAccountCredential): RuntimeProviderAccountCredential {
  if (stored.kind.kind === 'api_key') return { credential_revision: stored.credential_revision, identity_generation: stored.identity_generation, kind: stored.kind };
  const session = codexAccessToken(stored.kind);
  return { credential_revision: stored.credential_revision, identity_generation: stored.identity_generation,
    kind: { kind: 'oauth', access_token: session.access_token, refresh_token: null,
      expires_at: { secs_since_epoch: session.expires_at, nanos_since_epoch: 0 }, scopes: stored.kind.scopes,
      issuer: stored.kind.issuer, subject: stored.kind.subject ?? null, organization_id: session.account_id ?? null, email: null, fedramp: session.fedramp } };
}

/** Broker slot adapter keeps the complete grant private to the Desktop authority. */
export function providerAccountSessionStore(broker: CodexSessionStore, identityGeneration?: (reference: string) => number | undefined, publishVersion?: (version: ProviderAccountCredentialVersion) => Promise<void>): CodexSessionStore {
  const accountSlot = (id: string) => /^acct\.[a-f0-9-]{36}$/.test(id);
  return {
    async resolve(id) {
      if (accountSlot(id) && await broker.resolve(`${id}.mutation`)) throw new Error('Codex 凭据更新尚未提交，请重新连接账号。');
      const value = await broker.resolve(id);
      if (!value || !accountSlot(id)) return value;
      const record = parseStoredCredential(value);
      if (record.kind?.kind !== 'oauth') throw new Error('Codex 登录凭据无效。');
      return JSON.stringify(parseCodexSession(record.kind));
    },
    async set(id, value, generation) {
      if (!accountSlot(id)) return broker.set(id, value);
      const previous = await broker.resolve(id);
      const record: ProviderAccountCredential | undefined = previous ? parseStoredCredential(previous) : undefined;
      const kind: ProviderAccountCredential['kind'] = { kind: 'oauth', issuer: 'https://auth.openai.com', scopes: ['openid', 'profile', 'email', 'offline_access'],
        ...parseCodexSession(JSON.parse(value)) };
      const nextGeneration = generation ?? record?.identity_generation ?? identityGeneration?.(id) ?? 1;
      const sameGrant = generation === undefined && record?.identity_generation === nextGeneration && JSON.stringify(record.kind) === JSON.stringify(kind);
      const next: ProviderAccountCredential = sameGrant ? record : { credential_revision: (record?.credential_revision ?? 0) + 1,
        identity_generation: nextGeneration, kind };
      if (!sameGrant) await broker.set(id, JSON.stringify(next));
      // Explicit authorization commits metadata before publishing its generation.
      // Ordinary rotation already has metadata and must publish before consumers.
      if (generation === undefined) await publishVersion?.({ accountId: id.slice('acct.'.length), identityGeneration: next.identity_generation, credentialRevision: next.credential_revision });

    },
    delete: id => broker.delete(id),
  };
}

/** Atomic creation fences refresh even if a stale process lock is reclaimed concurrently. */
export function withFileRefreshIntents(store: CodexSessionStore, directory: () => Promise<string>): CodexSessionStore {
  const isIntent = (id: string) => id.endsWith('.intent');
  const path = async (id: string) => {
    if (!/^[a-z0-9._-]{1,80}$/.test(id)) throw new Error('invalid refresh intent');
    const root = await directory(); await mkdir(root, { recursive: true, mode: 0o700 });
    return join(root, id);
  };
  return {
    async resolve(id) {
      if (!isIntent(id)) return store.resolve(id);
      try { return await readFile(await path(id), 'utf8'); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; }
    },
    async set(id, value, generation) {
      if (!isIntent(id)) return store.set(id, value, generation);
      const target = await path(id);
      const handle = await open(target, 'wx', 0o600);
      try { await handle.writeFile(value); await handle.sync(); } finally { await handle.close(); }
      const parent = await open(await directory(), 'r');
      try { await parent.sync(); } finally { await parent.close(); }
    },
    async delete(id) {
      if (!isIntent(id)) return store.delete(id);
      await rm(await path(id), { force: true });
    },
  };
}

interface AccountLockOwner { version: 2; pid: number; token: string }
interface AccountLockClaim { owner: AccountLockOwner; dev: number; ino: number }
const lockRecoveryRequired = () => new Error('Provider account lock needs recovery.');

async function readAccountLock(path: string): Promise<AccountLockClaim | undefined> {
  try {
    const entry = await lstat(path);
    if (!entry.isFile() || entry.size > 256 || (process.getuid && entry.uid !== process.getuid())
      || (process.platform !== 'win32' && (entry.mode & 0o077) !== 0)) throw lockRecoveryRequired();
    const file = await open(path, 'r');
    try {
      const actual = await file.stat();
      if (entry.dev !== actual.dev || entry.ino !== actual.ino) return undefined;
      const owner = JSON.parse(await file.readFile('utf8')) as AccountLockOwner;
      if (Object.keys(owner).sort().join(',') !== 'pid,token,version' || owner.version !== 2
        || !Number.isSafeInteger(owner.pid) || owner.pid <= 0
        || typeof owner.token !== 'string' || !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(owner.token)) throw lockRecoveryRequired();
      return { owner, dev: entry.dev, ino: entry.ino };
    } finally { await file.close(); }
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined;
    throw lockRecoveryRequired();
  }
}

const sameAccountLock = (a: AccountLockClaim, b: AccountLockClaim): boolean =>
  a.dev === b.dev && a.ino === b.ino && a.owner.pid === b.owner.pid && a.owner.token === b.owner.token;
const accountLockOwnerDead = (pid: number): boolean => {
  try { process.kill(pid, 0); return false; }
  catch (error) { return (error as NodeJS.ErrnoException).code === 'ESRCH'; }
};
async function publishAccountLock(candidate: string, path: string): Promise<boolean> {
  try { await link(candidate, path); return true; }
  catch (error) { if ((error as NodeJS.ErrnoException).code === 'EEXIST') return false; throw error; }
}
async function releaseAccountLock(path: string, claim: AccountLockClaim): Promise<void> {
  const current = await readAccountLock(path);
  if (!current || !sameAccountLock(current, claim)) throw lockRecoveryRequired();
  await unlink(path);
}

/**
 * Ephemeral process exclusion, not a durable credential commit. Owner bytes are
 * synced before atomic publication; directory names need only survive process
 * exit, not power loss. Durable refresh/mutation intents remain separate.
 */
export async function withProviderAccountLock<T>(root: string, accountId: string, operation: () => Promise<T>): Promise<T> {
  if (!/^[a-z0-9._-]{1,64}$/.test(accountId)) throw new Error('invalid account lock');
  await mkdir(root, { recursive: true, mode: 0o700 });
  const lock = join(root, accountId), recovery = `${lock}.reclaim`;
  const owner: AccountLockOwner = { version: 2, pid: process.pid, token: randomUUID() };
  const candidate = join(root, `.${accountId}.${owner.token}.tmp`);
  const deadline = Date.now() + 90_000;
  let candidateCreated = false;
  try {
    const file = await open(candidate, 'wx', 0o600);
    candidateCreated = true;
    try { await file.writeFile(JSON.stringify(owner)); await file.sync(); } finally { await file.close(); }
    const claim = await readAccountLock(candidate);
    if (!claim) throw lockRecoveryRequired();
    for (;;) {
      const recovering = await readAccountLock(recovery);
      if (recovering) {
        // Reclaimers never delete another reclaimer's gate. A crashed reclaimer
        // remains explicit recovery work rather than risking a successor owner.
        if (accountLockOwnerDead(recovering.owner.pid)) throw lockRecoveryRequired();
      } else if (await publishAccountLock(candidate, lock)) {
        try { return await operation(); } finally { await releaseAccountLock(lock, claim); }
      } else {
        const observed = await readAccountLock(lock);
        if (!observed) continue;
        if (accountLockOwnerDead(observed.owner.pid) && await publishAccountLock(candidate, recovery)) {
          try {
            const current = await readAccountLock(lock);
            if (current && sameAccountLock(current, observed) && accountLockOwnerDead(current.owner.pid)) await unlink(lock);
          } finally { await releaseAccountLock(recovery, claim); }
          continue;
        }
      }
      if (Date.now() >= deadline) throw new Error('Provider account refresh is busy.');
      await new Promise(resolve => setTimeout(resolve, 50));
    }
  } finally {
    if (candidateCreated) await unlink(candidate).catch(error => {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    });
  }
}

export class DesktopProviderAccounts {
  lastRouting?: ProviderAccountRouting;
  recordRouting(event: ProviderAccountRouting): boolean {
    if (event.status === 'selected') {
      const account = this.latestSnapshot?.accounts.find(a => a.accountId === event.selectedAccountId);
      if (!account || account.identityGeneration !== event.identityGeneration || account.configRevision !== event.configRevision) return false;
    }
    this.lastRouting = event;
    return true;
  }
  latestSnapshot?: ProviderAccountSnapshot;
  private queue: Promise<unknown> = Promise.resolve();
  private closing = false;
  private disposePromise?: Promise<void>;
  private readonly pending = new Set<Promise<unknown>>();
  private readonly flows = new Map<string, { abort: AbortController; operationId?: number }>();
  constructor(private readonly options: {
    configuredApiBinding?: (providerId: string, bindingId: string) => boolean | Promise<boolean>;
    providerDefinition?: (bindingId: string) => ProviderDefinition | undefined;
    accountHome?: () => Promise<string>;
    withAccountMutationLock?: <T>(reference: string, operation: () => Promise<T>) => Promise<T>;
    registry: (request: ProviderAccountCommand, credentialPresence?: string[]) => Promise<ProviderAccountSnapshot>;
    broker: ProviderCredentialBroker;
    tokens: CodexTokenManager;
    login: (signal: AbortSignal) => Promise<CodexOAuthSession>;
    changed: (snapshot: ProviderAccountSnapshot) => Promise<void>;
    publishVersions: (versions: readonly ProviderAccountCredentialVersion[]) => Promise<void>;
    probe: (account: ProviderAccountMetadata, credential: ProviderAccountCredential) => Promise<void>;
    onCredentialDiagnostic?: (event: { accountId: string; identityGeneration: number; stage: string; outcome: string; code?: string; healthReason?: string }) => void;
  }) {}
  private track<T>(operation: Promise<T>): Promise<T> {
    this.pending.add(operation);
    void operation.then(() => this.pending.delete(operation), () => this.pending.delete(operation));
    return operation;
  }
  dispose(): Promise<void> {
    if (this.disposePromise) return this.disposePromise;
    this.closing = true;
    this.cancel();
    this.options.tokens.stop();
    this.disposePromise = Promise.allSettled([...this.pending]).then(() => this.options.tokens.dispose());
    return this.disposePromise;
  }
  async snapshot(): Promise<ProviderAccountSnapshot> {
    let snapshot = await this.options.registry({ action: 'list', expectedRevision: 0 });
    try {
      let present = new Set<string>();
      let pendingRefresh = new Set<string>();
      for (let attempt = 0; attempt < 3; attempt++) {
        present = new Set((await this.options.broker.listStatus(snapshot.accounts.flatMap(a => [a.credentialRef.secretRef, `${a.credentialRef.secretRef}.mutation`])))
          .filter(row => row.configured).map(row => row.providerId));
        pendingRefresh = new Set((await Promise.all(snapshot.accounts.map(async account => await this.options.tokens.hasPendingCommit(account.credentialRef.secretRef) ? account.credentialRef.secretRef : undefined))).filter((reference): reference is string => reference !== undefined));
        const credentialPresence = snapshot.accounts.filter(account => present.has(account.credentialRef.secretRef) && !present.has(`${account.credentialRef.secretRef}.mutation`) && !pendingRefresh.has(account.credentialRef.secretRef)).map(account => account.accountId);
        const projected = await this.options.registry({ action: 'list', expectedRevision: snapshot.configRevision }, credentialPresence);
        if (projected.storeScope === snapshot.storeScope && projected.configRevision === snapshot.configRevision) { snapshot = projected; break; }
        snapshot = projected;
        if (attempt === 2) throw new Error('Account settings changed while reading credential presence.');
      }
      snapshot.health = snapshot.accounts.map(account => {
        const existing = snapshot.health.find(h => h.accountId === account.accountId && h.identityGeneration === account.identityGeneration);
        if (present.has(`${account.credentialRef.secretRef}.mutation`) || pendingRefresh.has(account.credentialRef.secretRef)) return { accountId: account.accountId, identityGeneration: account.identityGeneration, status: 'storage_unavailable' as const, reason: 'credential_update_pending', healthRevision: existing?.healthRevision ?? 0 };
        if (!account.enabled) return { accountId: account.accountId, identityGeneration: account.identityGeneration, status: 'disabled' as const, healthRevision: existing?.healthRevision ?? 0 };
        if (!present.has(account.credentialRef.secretRef)) return { accountId: account.accountId, identityGeneration: account.identityGeneration, status: 'needs_auth' as const, reason: 'credential_missing', healthRevision: existing?.healthRevision ?? 0 };
        return existing ?? { accountId: account.accountId, identityGeneration: account.identityGeneration, status: 'ready' as const, healthRevision: 0 };
      });
    } catch {
      snapshot.modelAvailability = snapshot.modelAvailability?.map(entry => ({ ...entry, eligibleAccountIds: [] }));
      snapshot.health = snapshot.accounts.map(a => ({ accountId: a.accountId, identityGeneration: a.identityGeneration,
        status: 'storage_unavailable', reason: 'secure_storage_unavailable', healthRevision: 0 }));
    }
    this.latestSnapshot = snapshot;
    return snapshot;
  }
  cancel(target?: string | number): void { for (const [id, flow] of this.flows) if (target === undefined || id === target || flow.operationId === target) flow.abort.abort(); }
  command(request: ProviderAccountCommand, credential?: string, signal?: AbortSignal): Promise<ProviderAccountSnapshot> {
    if (this.closing) return Promise.reject(new Error('Provider accounts are shutting down.'));
    const submitted = request.action === 'add' ? { ...request, accountId: randomUUID() } : request;
    const result = this.queue.catch(() => undefined).then(async () => {
      if (this.closing) throw new Error('Provider accounts are shutting down.');
      if (signal?.aborted) throw new Error('Account operation cancelled.');
      const run = () => this.execute(submitted, credential, Boolean(this.options.withAccountMutationLock), signal);
      let snapshot: ProviderAccountSnapshot;
      if (this.options.withAccountMutationLock && submitted.accountId) {
        const current = await this.options.registry({ action: 'list', expectedRevision: 0 });
        const reference = current.accounts.find(a => a.accountId === submitted.accountId)?.credentialRef.secretRef ?? `acct.${submitted.accountId}`;
        snapshot = await this.options.withAccountMutationLock(reference, run);
      } else snapshot = await this.execute(submitted, credential, false, signal);
      if (submitted.action !== 'list') await this.options.changed(snapshot);
      return snapshot;
    });
    this.queue = result;
    return this.track(result);
  }
  private async execute(request: ProviderAccountCommand, credential: string | undefined, lockHeld: boolean, signal?: AbortSignal): Promise<ProviderAccountSnapshot> {
    if (signal?.aborted) throw new Error('Account operation cancelled.');
    const before = await this.snapshot();
    if (request.action === 'list') return before;
    if (before.configRevision !== request.expectedRevision) throw new Error('Account settings changed. Refresh and retry.');
    const account = before.accounts.find(a => a.accountId === request.accountId);
    if (request.action !== 'add' && request.action !== 'reorder' && request.action !== 'set_policy' && !account) throw new Error('Provider account no longer exists.');
    if (request.action === 'test') {
      if (account!.authKind === 'oauth') await this.options.tokens.current(account!.credentialRef.secretRef, lockHeld);
      const secret = await this.options.broker.resolve(account!.credentialRef.secretRef);
      if (!secret) throw new Error('Provider account needs authentication.');
      let record = parseStoredCredential(secret);
      if (record.identity_generation !== account!.identityGeneration) throw new Error('Reconnect this account before testing.');
      await this.options.probe(account!, record);
      return this.snapshot();
    }
    if (request.action === 'add' || request.action === 'reconnect') {
      const accountId = account?.accountId ?? request.accountId ?? randomUUID();
      const secretRef = account?.credentialRef.secretRef ?? `acct.${accountId}`;
      let metadataCommitted: ProviderAccountSnapshot | undefined;
      const previousSecret = await this.options.broker.resolve(secretRef);
      const previousRecord: ProviderAccountCredential | undefined = previousSecret ? parseStoredCredential(previousSecret) : undefined;
      const authKind = account?.authKind ?? request.authKind;
      const providerId = logicalProviderId(account?.providerId ?? request.providerId ?? '');
      const binding = account?.executionBindingId ?? request.executionBindingId;
      const definition = this.options.providerDefinition?.(binding ?? '') ?? providerById(binding ?? '');
      const configuredKeyBinding = authKind === 'api_key' && await this.options.configuredApiBinding?.(providerId, binding ?? '');
      if (!configuredKeyBinding && (!definition?.available || logicalProviderId(definition.id) !== providerId
        || (authKind === 'oauth') !== (definition.id === CODEX_PROVIDER_ID))) throw new Error('Authentication method is unavailable for this provider.');
      const mutationRef = `${secretRef}.mutation`;
      let mutationStarted = false;
      try {
        if (authKind === 'oauth') {
          if (this.closing) throw new Error('Provider accounts are shutting down.');
          if (providerId !== 'openai' || (account?.executionBindingId ?? request.executionBindingId) !== CODEX_PROVIDER_ID) throw new Error('OAuth is unavailable for this provider.');
          const abort = new AbortController();
          const cancel = () => abort.abort();
          signal?.addEventListener('abort', cancel, { once: true });
          if (signal?.aborted) abort.abort();
          this.flows.set(accountId, { abort, operationId: request.operationId });
          try {
            const session = await this.options.login(abort.signal);
            if (abort.signal.aborted) throw new Error('Sign-in cancelled.');
            const latest = await this.snapshot();
            if (latest.configRevision !== before.configRevision) throw new Error('Account settings changed during sign-in. Retry.');
            if (!session.subject) throw new Error('The provider did not return a stable account identity.');
            for (const other of latest.accounts.filter(a => a.authKind === 'oauth')) {
              const raw = await this.options.broker.resolve(other.credentialRef.secretRef);
              const stored = raw ? parseStoredCredential(raw) : undefined;
              if (stored?.kind.kind !== 'oauth') continue;
              if (other.accountId === accountId && ((stored.kind.subject && stored.kind.subject !== session.subject) || stored.kind.account_id !== session.account_id)) throw new Error('This is a different account. Add it as a new account.');
              if (other.accountId !== accountId && stored.kind.subject === session.subject && stored.kind.account_id === session.account_id) throw new Error('This ChatGPT account is already added.');
            }
            await this.options.broker.set(mutationRef, JSON.stringify({ operation: randomUUID(), expectedRevision: before.configRevision, identityGeneration: account?.identityGeneration ?? 1 }));
            mutationStarted = true;
            await this.options.tokens.signIn(session, secretRef, account ? account.identityGeneration + 1 : 1, lockHeld, async () => {
              metadataCommitted = account
                ? await this.options.registry({ action: 'set_credential', expectedRevision: before.configRevision, accountId })
                : await this.options.registry({ ...request, action: 'add', accountId, providerId, authKind,
                    displayName: request.displayName?.trim() || `${providerId} ChatGPT` });
              const committed = parseStoredCredential((await this.options.broker.resolve(secretRef))!);
              await this.options.publishVersions([{ accountId, identityGeneration: committed.identity_generation, credentialRevision: committed.credential_revision }]);
              await this.options.broker.delete(mutationRef);
            });
          } finally { signal?.removeEventListener('abort', cancel); if (this.flows.get(accountId)?.abort === abort) this.flows.delete(accountId); }
        } else {
          if (!credential?.trim() || credential.length > 12_000 || credential.includes('\0')) throw new Error('Enter a valid API key.');
          await this.options.broker.set(mutationRef, JSON.stringify({ operation: randomUUID(), expectedRevision: before.configRevision, identityGeneration: (account?.identityGeneration ?? 0) + 1 }));
          mutationStarted = true;
          const nextGeneration = account ? account.identityGeneration + 1 : 1;
          await this.options.broker.set(secretRef, JSON.stringify({ credential_revision: (previousRecord?.credential_revision ?? 0) + 1,
            identity_generation: nextGeneration, kind: { kind: 'api_key', key: credential.trim() } } satisfies ProviderAccountCredential));
          if (account) metadataCommitted = await this.options.registry({ action: 'set_credential', expectedRevision: before.configRevision, accountId });
        }
        const snapshot: ProviderAccountSnapshot = metadataCommitted ?? await this.options.registry({ ...request, action: request.action === 'reconnect' ? 'update' : 'add', accountId, providerId,
            authKind, displayName: request.displayName?.trim() || account?.displayName || `${providerId} ${authKind === 'oauth' ? 'ChatGPT' : 'API'}` });
        metadataCommitted = snapshot;
        if (authKind !== 'oauth') {
          const committed = parseStoredCredential((await this.options.broker.resolve(secretRef))!);
          await this.options.publishVersions([{ accountId, identityGeneration: committed.identity_generation, credentialRevision: committed.credential_revision }]);
          await this.options.broker.delete(mutationRef);
        }
        // A runtime delivery failure must never roll back a committed account.
        if (authKind === 'oauth') this.options.tokens.watch(secretRef);
        return this.snapshot();
      } catch (error) {
        const latest = await this.options.registry({ action: 'list', expectedRevision: 0 }).catch(() => undefined);
        const retained = latest?.accounts.find(a => a.accountId === accountId);
        // Restore only before metadata commit. A committed operation survives
        // delivery errors, and an interrupted commit leaves its durable marker.
        if (mutationStarted && !metadataCommitted && latest && account && retained?.configRevision === account.configRevision) {
          if (previousSecret) await this.options.broker.set(secretRef, previousSecret);
          await this.options.broker.delete(mutationRef);
          this.options.tokens.invalidate(secretRef);
        } else if (mutationStarted && latest && !latest.accounts.some(a => a.credentialRef.secretRef === secretRef)) {
          await this.options.tokens.signOut(secretRef, lockHeld);
          await this.options.broker.delete(mutationRef);
        }
        throw error;
      }
    }
    const deleting = request.action === 'remove' || request.action === 'logout';
    const deletionMarker = account ? `${account.credentialRef.secretRef}.mutation` : undefined;
    if (deleting && deletionMarker) await this.options.broker.set(deletionMarker, JSON.stringify({ operation: randomUUID(), action: request.action, accountId: account!.accountId, expectedRevision: before.configRevision }));
    let result: ProviderAccountSnapshot;
    try { result = await this.options.registry(request); }
    catch (error) { if (deleting && deletionMarker) await this.options.broker.delete(deletionMarker); throw error; }
    if (request.action === 'remove' || request.action === 'logout') {
      this.cancel(account!.accountId);
      if (!result.accounts.some(a => a.accountId !== account!.accountId && a.credentialRef.secretRef === account!.credentialRef.secretRef)) {
        if (account!.authKind === 'oauth') await this.options.tokens.signOut(account!.credentialRef.secretRef, lockHeld);
        else await this.options.broker.delete(account!.credentialRef.secretRef);
      }
      await this.options.broker.delete(`${account!.credentialRef.secretRef}.mutation`);
    }
    return this.snapshot();
  }
  /** Reload the canonical account while sharing the mutation/rotation lock. */
  renewForRuntime(accountId: string, identityGeneration: number, rejectedRevision: number): Promise<void> {
    if (this.closing) return Promise.reject(new Error('Provider accounts are shutting down.'));
    return this.track(this.renewRuntime(accountId, identityGeneration, rejectedRevision));
  }
  private async renewRuntime(accountId: string, identityGeneration: number, rejectedRevision: number): Promise<void> {
    let stage: Exclude<ProviderAccountRenewFailure, 'authority_unavailable'> = 'snapshot_failed';
    try {
      const initial = await this.snapshot();
      const reference = initial.accounts.find(account => account.accountId === accountId)?.credentialRef.secretRef;
      if (!reference) throw new ProviderAccountRenewError('account_missing', 'Provider account no longer exists.');
      const run = async () => {
        stage = 'snapshot_failed';
        const snapshot = await this.snapshot();
        const account = snapshot.accounts.find(candidate => candidate.accountId === accountId && candidate.enabled && candidate.identityGeneration === identityGeneration);
        if (!account || account.credentialRef.secretRef !== reference) throw new ProviderAccountRenewError('identity_changed', 'Provider account identity changed.');
        stage = 'pending_check_failed';
        if (await this.options.broker.resolve(`${reference}.mutation`) || await this.options.tokens.hasPendingCommit(reference)) throw new ProviderAccountRenewError('storage_pending', 'Provider account storage needs recovery.');
        stage = 'broker_read_failed';
        const raw = await this.options.broker.resolve(reference);
        if (!raw) throw new ProviderAccountRenewError('credential_missing', 'Provider account needs authentication.');
        stage = 'decode_failed';
        const credential = parseStoredCredential(raw);
        if (credential.identity_generation !== identityGeneration) throw new ProviderAccountRenewError('identity_changed', 'Provider account identity changed.');
        if (credential.credential_revision > rejectedRevision) return;
        if (credential.kind.kind !== 'oauth') throw new ProviderAccountRenewError('api_key_rejected', 'Provider account credentials changed.');
        if (credential.credential_revision !== rejectedRevision) throw new ProviderAccountRenewError('revision_changed', 'Provider account credentials changed.');
        stage = 'token_refresh_failed';
        await this.options.tokens.renew(reference, credential.kind.access_token, Boolean(this.options.withAccountMutationLock));
      };
      stage = 'lock_failed';
      if (this.options.withAccountMutationLock) await this.options.withAccountMutationLock(reference, run);
      else await run();
    } catch (error) {
      if (error instanceof ProviderAccountRenewError) throw error;
      throw new ProviderAccountRenewError(stage, 'Provider account refresh failed.');
    }
  }

  launch(): Promise<ProviderAccountLaunch> {
    if (this.closing) return Promise.reject(new Error('Provider accounts are shutting down.'));
    return this.track(this.loadForRuntime());
  }
  /** Load one saved API-key binding without probing or refreshing other accounts. */
  launchApiKeyAccount(accountId: string, providerId: string, executionBindingId: string): Promise<ProviderAccountLaunch> {
    if (this.closing) return Promise.reject(new Error('Provider accounts are shutting down.'));
    return this.track(this.loadForRuntime({ accountId, providerId, executionBindingId }));
  }
  private async loadForRuntime(selected?: Pick<ProviderAccountMetadata, 'accountId' | 'providerId' | 'executionBindingId'>): Promise<ProviderAccountLaunch> {
    const providerAccounts = selected ? await this.options.registry({ action: 'list', expectedRevision: 0 }) : await this.snapshot();
    const providerAccountCredentials: Record<string, RuntimeProviderAccountCredential> = Object.create(null);
    const eligible = (account: ProviderAccountMetadata) => account.enabled && (!selected || account.accountId === selected.accountId
      && account.providerId === selected.providerId && account.executionBindingId === selected.executionBindingId && account.authKind === 'api_key');
    for (const account of providerAccounts.accounts.filter(eligible)) {
      if (providerAccounts.health.find(h => h.accountId === account.accountId)?.reason === 'credential_update_pending') {
        this.options.onCredentialDiagnostic?.({ accountId: account.accountId, identityGeneration: account.identityGeneration, stage: 'snapshot', outcome: 'skipped', healthReason: 'credential_update_pending' });
        continue;
      }
      let stage = 'lock';
      let versionCommitFailed = false;
      try {
        const load = async (lockHeld: boolean) => {
          const reference = account.credentialRef.secretRef;
          if (selected) {
            stage = 'identity';
            const latest = await this.options.registry({ action: 'list', expectedRevision: 0 });
            const current = latest.accounts.find(a => a.accountId === account.accountId);
            if (latest.storeScope !== providerAccounts.storeScope || !current || !eligible(current)
              || current.identityGeneration !== account.identityGeneration || current.credentialRef.secretRef !== reference) {
              throw new Error('Provider account changed while loading credentials.');
            }
          }
          stage = 'pending_check';
          if (await this.options.broker.resolve(`${reference}.mutation`) || await this.options.tokens.hasPendingCommit(reference)) throw new Error('Credential commit is pending.');
          stage = 'broker_read';
          const raw = await this.options.broker.resolve(reference);
          if (!raw) { this.options.onCredentialDiagnostic?.({ accountId: account.accountId, identityGeneration: account.identityGeneration, stage, outcome: 'missing' }); return; }
          stage = 'decode';
          let stored = parseStoredCredential(raw);
          stage = 'identity';
          if (stored.identity_generation !== account.identityGeneration) throw new Error('Credential identity changed. Reconnect this account.');
          if (selected && stored.kind.kind !== 'api_key') throw new Error('The selected account no longer has an API key.');
          if (stored.kind.kind === 'oauth') {
            stage = 'token_refresh';
            const session = await this.options.tokens.current(reference, lockHeld);
            if (!session) { this.options.onCredentialDiagnostic?.({ accountId: account.accountId, identityGeneration: account.identityGeneration, stage, outcome: 'missing' }); return; }
            stage = 'broker_read';
            const persisted = await this.options.broker.resolve(reference);
            stage = 'decode';
            const latest = persisted ? parseStoredCredential(persisted) : stored;
            stored = { ...latest, kind: { ...stored.kind, ...session } };
          }
          stage = 'publish_version';
          try { await this.options.publishVersions([{ accountId: account.accountId, identityGeneration: stored.identity_generation, credentialRevision: stored.credential_revision }]); }
          catch {
            versionCommitFailed = true;
            stage = 'persist_recovery_marker';
            await this.options.broker.set(`${reference}.mutation`, JSON.stringify({ operation: 'publish_credential_version', identityGeneration: stored.identity_generation, credentialRevision: stored.credential_revision }));
            throw new Error('Credential version publication is pending.');
          }
          stage = 'projection';
          providerAccountCredentials[account.accountId] = runtimeAccountCredential(stored);
          this.options.onCredentialDiagnostic?.({ accountId: account.accountId, identityGeneration: account.identityGeneration, stage, outcome: 'included' });
        };
        if (this.options.withAccountMutationLock) await this.options.withAccountMutationLock(account.credentialRef.secretRef, () => load(true));
        else await load(false);
      } catch (error) {
        const health = providerAccounts.health.find(h => h.accountId === account.accountId);
        if (health) {
          const transient = error instanceof Error && error.message.includes('HTTP');
          const pending = versionCommitFailed || await this.options.tokens.hasPendingCommit(account.credentialRef.secretRef).catch(() => true);
          health.status = pending ? 'storage_unavailable' : transient ? 'cooling_down' : account.authKind === 'oauth' ? 'needs_auth' : 'storage_unavailable';
          health.reason = pending ? 'credential_update_pending' : transient ? 'oauth_refresh_temporarily_unavailable' : 'credential_unavailable';
          if (transient) health.cooldownUntilMs = Date.now() + 60_000;
        }
        const rawCode = (error as NodeJS.ErrnoException | undefined)?.code;
        const lockFailures: Record<string, string> = {
          'Provider account lock needs recovery.': 'LOCK_RECOVERY_REQUIRED',
          'Provider account refresh is busy.': 'LOCK_BUSY',
          'invalid account lock': 'INVALID_ACCOUNT_LOCK',
          'session catalog timed out': 'PRODUCT_PATHS_TIMEOUT',
          'invalid product paths response': 'INVALID_PRODUCT_PATHS',
        };
        const code = typeof rawCode === 'string' && ['ENOENT', 'EACCES', 'EPERM', 'ETIMEDOUT', 'EPIPE', 'ECONNRESET'].includes(rawCode) ? rawCode
          : stage === 'lock' && error instanceof Error ? lockFailures[error.message] : undefined;
        this.options.onCredentialDiagnostic?.({ accountId: account.accountId, identityGeneration: account.identityGeneration, stage, outcome: 'failed', ...(code ? { code } : {}), ...(health?.reason ? { healthReason: health.reason } : {}) });
      }
    }
    return { providerAccounts, providerAccountCredentials, providerAccountHome: selected ? undefined : await this.options.accountHome?.() };
  }
}
