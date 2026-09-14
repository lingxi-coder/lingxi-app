import type { CustomProviderConnectionDraft, CustomProviderDraft } from './customProviderImport';

/** Normalize only editable strings; retain capabilities and other advanced configuration. */
export function trimProviderDraft(draft: CustomProviderDraft): CustomProviderDraft {
  return {
    ...draft,
    ...(typeof draft.baseUrl === 'string' ? { baseUrl: draft.baseUrl.trim() } : {}),
    ...(typeof draft.apiKeyEnv === 'string' ? { apiKeyEnv: draft.apiKeyEnv.trim() } : {}),
    models: Array.isArray(draft.models) ? draft.models.map((model) => (
      model && typeof model === 'object' ? { ...model, ...(typeof model.id === 'string' ? { id: model.id.trim() } : {}) } : model
    )) : draft.models,
    // Connections carry the same editable strings as the provider row, so they
    // need the same normalization; a stray space in a connection's id would
    // otherwise reach the engine and change the profile name it desugars to.
    ...(Array.isArray(draft.connections) ? {
      connections: draft.connections.map((connection) => (
        connection && typeof connection === 'object' ? {
          ...connection,
          ...(typeof connection.id === 'string' ? { id: connection.id.trim() } : {}),
          ...(typeof connection.baseUrl === 'string' ? { baseUrl: connection.baseUrl.trim() } : {}),
          ...(typeof connection.apiKeyEnv === 'string' ? { apiKeyEnv: connection.apiKeyEnv.trim() } : {}),
        } : connection
      )),
    } : {}),
  };
}

export function editableProviderDraft(value: unknown): CustomProviderDraft {
  const raw = value && typeof value === 'object' && !Array.isArray(value) ? value as CustomProviderDraft : { type: 'openai', models: [] };
  return { ...structuredClone(raw), type: typeof raw.type === 'string' ? raw.type : 'openai', models: Array.isArray(raw.models) ? raw.models.map((model) => (
    typeof model === 'string' ? { id: model } : model && typeof model === 'object' ? { ...model, id: typeof model.id === 'string' ? model.id : '' } : { id: '' }
  )) : [{ id: '' }] };
}

/** The caller retains pricingId while an input is blank/duplicate so a later edit can recover it. */
export function renameProviderModel(draft: CustomProviderDraft, index: number, id: string, pricingId = (typeof draft.models[index]?.id === 'string' ? draft.models[index].id.trim() : '')): { draft: CustomProviderDraft; pricingId: string } {
  const models = draft.models.map((model, i) => i === index ? { ...model, id } : model);
  const target = id.trim();
  if (!target || models.some((model, i) => i !== index && typeof model.id === 'string' && model.id.trim() === target)) return { draft: { ...draft, models }, pricingId };
  const pricing = draft.pricing;
  if (!pricing || typeof pricing !== 'object' || Array.isArray(pricing) || pricingId === target) return { draft: { ...draft, models }, pricingId: target };
  const prices = pricing as Record<string, unknown>;
  if (!Object.prototype.hasOwnProperty.call(prices, pricingId)) return { draft: { ...draft, models }, pricingId: target };
  // A target may already own a different price; never silently replace it.
  if (Object.prototype.hasOwnProperty.call(prices, target)) return { draft: { ...draft, models }, pricingId };
  const next = { ...prices, [target]: prices[pricingId] };
  if (!models.some(model => typeof model.id === 'string' && model.id.trim() === pricingId)) delete next[pricingId];
  return { draft: { ...draft, models, pricing: next }, pricingId: target };
}

/** A row temporarily borrowing another ID must be resolved before deleting that target. */
export function canRemoveProviderModel(draft: CustomProviderDraft, index: number, pricingIds: readonly string[]): boolean {
  const target = pricingIds[index];
  return !target || !draft.models.some((model, i) => i !== index && typeof model.id === 'string' && model.id.trim() === target && pricingIds[i] !== target);
}

export function removeProviderModel(draft: CustomProviderDraft, index: number, pricingId = (typeof draft.models[index]?.id === 'string' ? draft.models[index].id.trim() : ''), pricingIds?: readonly string[]): CustomProviderDraft {
  if (pricingIds && !canRemoveProviderModel(draft, index, pricingIds)) return draft;
  const models = draft.models.filter((_, i) => i !== index);
  const pricing = draft.pricing;
  if (!pricing || typeof pricing !== 'object' || Array.isArray(pricing) || models.some(model => typeof model.id === 'string' && model.id.trim() === pricingId)) return { ...draft, models };
  const next = { ...pricing } as Record<string, unknown>;
  delete next[pricingId];
  return { ...draft, models, pricing: next };
}

// ── Connections ─────────────────────────────────────────────────────────────
// A provider reachable several ways (a domestic and an international host, or
// several API keys) holds a `connections` list; each entry inherits every
// provider-level field it does not restate. These helpers are pure so the
// rules can be tested without mounting the editor.

/** The connection list, or `[]` for a provider that declares none. */
export function providerConnections(draft: CustomProviderDraft): CustomProviderConnectionDraft[] {
  return Array.isArray(draft.connections) ? draft.connections : [];
}

/**
 * Add a connection.
 *
 * The FIRST call migrates the flat provider into two connections: the existing
 * configuration becomes `default` so nothing the user already set is lost, and
 * the new one starts empty. `baseUrl` moves down onto `default` rather than
 * being left at provider level, so editing the new connection's URL cannot
 * silently inherit the old one.
 */
export function addProviderConnection(draft: CustomProviderDraft, id = ''): CustomProviderDraft {
  const existing = providerConnections(draft);
  if (existing.length) return { ...draft, connections: [...existing, { id }] };
  const seeded: CustomProviderConnectionDraft = { id: 'default' };
  if (typeof draft.baseUrl === 'string') seeded.baseUrl = draft.baseUrl;
  if (typeof draft.apiKeyEnv === 'string') seeded.apiKeyEnv = draft.apiKeyEnv;
  const next = { ...draft, connections: [seeded, { id }] };
  delete next.baseUrl;
  delete next.apiKeyEnv;
  return next;
}

/** Patch one connection, dropping keys whose value is `undefined`. */
export function updateProviderConnection(draft: CustomProviderDraft, index: number, patch: Record<string, unknown>): CustomProviderDraft {
  const connections = providerConnections(draft).map((connection, i) => {
    if (i !== index) return connection;
    const next = { ...connection, ...patch };
    for (const [key, value] of Object.entries(patch)) if (value === undefined) delete next[key];
    return next;
  });
  return { ...draft, connections };
}

/**
 * Remove a connection.
 *
 * Removing the last-but-one collapses back to a flat provider, lifting the
 * survivor's fields to provider level — otherwise a one-entry `connections`
 * list would keep claiming the provider is reachable several ways.
 */
export function removeProviderConnection(draft: CustomProviderDraft, index: number): CustomProviderDraft {
  const remaining = providerConnections(draft).filter((_, i) => i !== index);
  if (remaining.length > 1) return { ...draft, connections: remaining };
  const next = { ...draft };
  delete next.connections;
  delete next.fallback;
  if (!remaining.length) return next;
  const [survivor] = remaining;
  for (const [key, value] of Object.entries(survivor)) if (key !== 'id') (next as Record<string, unknown>)[key] = value;
  return next;
}
