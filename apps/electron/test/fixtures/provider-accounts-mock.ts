import type { ProviderAccountCommand, ProviderAccountMetadata, ProviderAccountSnapshot, ProviderAccountsApi } from '../../src/shared/providerAccounts';

/** Synthetic in-memory authority for renderer interaction tests only. */
export function providerAccountsMock(options: {
  initial?: ProviderAccountMetadata[];
  write?: (providerId: string, credential: string) => Promise<void>;
  probe?: () => Promise<void>;
  changed?: (snapshot: ProviderAccountSnapshot) => void;
} = {}): ProviderAccountsApi {
  let state: ProviderAccountSnapshot = { storeScope: 'renderer-fixture', configRevision: 0, accounts: options.initial ?? [], health: [], modelEntitlements: [], modelAvailability: [] };
  const snapshot = () => structuredClone({ ...state, health: state.accounts.map(account => ({ accountId: account.accountId, identityGeneration: account.identityGeneration, status: account.enabled ? 'ready' as const : 'disabled' as const, healthRevision: 0 })) });
  return {
    snapshot: async () => snapshot(), cancel: async () => {},
    command: async (request: ProviderAccountCommand, credential?: string) => {
      if (request.expectedRevision !== state.configRevision) throw new Error('Account settings changed.');
      if (request.action === 'test') { await options.probe?.(); return snapshot(); }
      if (request.action === 'list') return snapshot();
      const account = state.accounts.find(account => account.accountId === request.accountId);
      if (request.action === 'add' || request.action === 'reconnect') {
        if (request.authKind === 'oauth') throw new Error('OAuth is not mocked by this renderer fixture.');
        await options.write?.(account?.providerId ?? request.providerId!, credential!);
        if (request.action === 'add') { const accountId = crypto.randomUUID(); state.accounts.push({ accountId, providerId: request.providerId!, executionBindingId: request.executionBindingId!, displayName: request.displayName || request.providerId!, authKind: 'api_key', enabled: true, userPriority: state.accounts.length, configRevision: state.configRevision + 1, identityGeneration: 1, credentialRef: { storeScope: state.storeScope, secretRef: `acct.${accountId}` } }); }
        else { if (!account) throw new Error('Account missing'); account.identityGeneration++; }
      } else if (request.action === 'remove') state.accounts = state.accounts.filter(account => account.accountId !== request.accountId);
      else if (request.action === 'enable' && account) account.enabled = request.enabled!;
      else if (request.action === 'update' && account) account.displayName = request.displayName!;
      else if (request.action === 'reorder') state.accounts.forEach(account => { account.userPriority = request.accountIds!.indexOf(account.accountId); });
      else throw new Error(`Unsupported renderer fixture operation: ${request.action}`);
      state = { ...state, configRevision: state.configRevision + 1 };
      const next = snapshot(); options.changed?.(next); return next;
    },
  };
}

export function fixtureAccountSnapshot(providerIds: string[], modelReferences = providerIds.map(id => `${id}/fixture-model`)): ProviderAccountSnapshot {
  const accounts: ProviderAccountMetadata[] = providerIds.map((binding, index) => {
    const accountId = `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`;
    return { accountId, providerId: binding === 'openai-chatgpt' ? 'openai' : binding, executionBindingId: binding,
      displayName: `Fixture ${binding}`, authKind: binding === 'openai-chatgpt' ? 'oauth' : 'api_key', enabled: true,
      userPriority: index, configRevision: 1, identityGeneration: 1, credentialRef: { storeScope: 'renderer-fixture', secretRef: `acct.${accountId}` } };
  });
  return { storeScope: 'renderer-fixture', configRevision: 1, accounts, health: accounts.map(account => ({ accountId: account.accountId, identityGeneration: 1, status: 'ready', healthRevision: 0 })), modelEntitlements: [], modelAvailability: modelReferences.flatMap(reference => { const index = reference.indexOf('/'); const binding = reference.slice(0, index); const account = accounts.find(row => row.executionBindingId === binding); return account ? [{ providerId: account.providerId, executionBindingId: binding, modelId: reference.slice(index + 1), eligibleAccountIds: [account.accountId], confirmedAccountIds: [], unverifiedAccountIds: [account.accountId], forbiddenAccountIds: [] }] : []; }) };
}
