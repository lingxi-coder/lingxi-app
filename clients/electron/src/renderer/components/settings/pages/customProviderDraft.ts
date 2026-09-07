import type { CustomProviderDraft } from './customProviderImport';

/** Normalize only editable strings; retain capabilities and other advanced configuration. */
export function trimProviderDraft(draft: CustomProviderDraft): CustomProviderDraft {
  return {
    ...draft,
    ...(typeof draft.baseUrl === 'string' ? { baseUrl: draft.baseUrl.trim() } : {}),
    ...(typeof draft.apiKeyEnv === 'string' ? { apiKeyEnv: draft.apiKeyEnv.trim() } : {}),
    models: Array.isArray(draft.models) ? draft.models.map((model) => (
      model && typeof model === 'object' ? { ...model, ...(typeof model.id === 'string' ? { id: model.id.trim() } : {}) } : model
    )) : draft.models,
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
