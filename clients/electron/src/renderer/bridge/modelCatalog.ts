import type { ModelDetailsDto, ProviderModelCatalogEntryDto } from '@lingxi/bridge-client';
import { providerById } from '../../shared/providers';
import type {
  ModelPickerVisibilitySettings,
  ProviderModelPickerVisibility,
} from '../../shared/settings';

export interface ModelReference {
  readonly reference: string;
  readonly providerId: string | null;
  readonly requestModel: string;
  readonly label: string;
}

export interface ModelProviderGroup {
  readonly providerId: string | null;
  readonly providerLabel: string;
  readonly models: readonly ModelReference[];
}

export interface ModelCatalogDetail {
  readonly reference: string;
  readonly display_name?: string;
  readonly pricing?: { readonly billing_mode?: string } | null;
}

export interface VisibleProviderCatalog {
  readonly providerId: string;
  readonly displayName: string;
  readonly models: readonly ModelDetailsDto[];
}

export interface ModelBillingGroup {
  readonly label: 'Paid' | 'Free' | null;
  readonly models: readonly ModelReference[];
}

export function modelCapabilitySummary(model: ModelDetailsDto): string {
  const capabilities = model.capabilities;
  const labels = [
    capabilities?.tools ? 'Tools' : null,
    capabilities?.vision ? 'Vision' : null,
    capabilities?.documents ? 'Documents' : null,
    capabilities?.reasoning ? 'Reasoning' : null,
    capabilities?.structured_output ? 'Structured output' : null,
    model.attachments ? 'Attachments' : null,
  ].filter((label): label is string => label !== null);
  if (model.context_window_tokens) {
    labels.push(`${Math.round(model.context_window_tokens / 1_000)}k context`);
  }
  return labels.join(' · ');
}

export type ModelSelectionDecision =
  | { kind: 'select'; reference: string }
  | { kind: 'connect'; providerId: string; reference: string }
  | { kind: 'loading'; providerId: string; reference: string };

export function modelSelectionConfirmed(currentModel: string | null, requestedModel: string): boolean {
  return currentModel === requestedModel;
}

export async function waitForModelSelection(
  requestedModel: string,
  readCurrentModel: () => string | null,
  options: { timeoutMs?: number; pollMs?: number; signal?: AbortSignal } = {},
): Promise<void> {
  const timeoutMs = options.timeoutMs ?? 5_000;
  const pollMs = options.pollMs ?? 50;
  if (modelSelectionConfirmed(readCurrentModel(), requestedModel)) return;
  if (options.signal?.aborted) return Promise.reject(new Error('model selection wait aborted'));

  await new Promise<void>((resolve, reject) => {
    const startedAt = Date.now();
    let settled = false;
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      clearInterval(timer);
      clearTimeout(timeout);
      options.signal?.removeEventListener('abort', onAbort);
      if (error) reject(error);
      else resolve();
    };
    const onAbort = () => finish(new Error('model selection wait aborted'));
    const timer = setInterval(() => {
      if (options.signal?.aborted) return onAbort();
      if (modelSelectionConfirmed(readCurrentModel(), requestedModel)) {
        finish();
      } else if (Date.now() - startedAt >= timeoutMs) {
        finish(new Error(`The engine did not confirm model ${requestedModel} in time.`));
      }
    }, pollMs);
    const timeout = setTimeout(() => {
      finish(new Error(`The engine did not confirm model ${requestedModel} in time.`));
    }, timeoutMs);
    options.signal?.addEventListener('abort', onAbort, { once: true });
  });
}

/**
 * Resolve a model click without coupling the picker to credential storage.
 * Unknown provider prefixes are engine-authoritative and remain selectable;
 * only providers with a desktop credential flow can open Settings.
 */
export function resolveModelSelection(
  reference: string,
  credentials?: readonly { providerId: string; configured: boolean }[],
): ModelSelectionDecision {
  const parsed = modelReference(reference);
  if (!parsed.providerId) return { kind: 'select', reference };

  const providerId = parsed.providerId === 'builtin' ? 'anthropic' : parsed.providerId;
  if (!providerById(providerId)) return { kind: 'select', reference };
  if (!credentials) return { kind: 'loading', providerId, reference };

  const metadata = credentials.find((entry) => entry.providerId === providerId);
  return metadata?.configured
    ? { kind: 'select', reference }
    : { kind: 'connect', providerId, reference };
}

function titleCase(value: string): string {
  return value
    .replace(/[-_]/g, ' ')
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function modelLabel(requestModel: string): string {
  const leaf = requestModel.split('/').at(-1) ?? requestModel;
  const parts = leaf
    .split(/[-_]/)
    .filter(Boolean)
    .filter((part, index, all) => !(index === all.length - 1 && /^\d{6,}$/.test(part)));
  return parts.map((part) => {
    switch (part.toLowerCase()) {
      case 'gpt': return 'GPT';
      case 'glm': return 'GLM';
      case 'deepseek': return 'DeepSeek';
      case 'kimi': return 'Kimi';
      case 'gemini': return 'Gemini';
      case 'claude': return 'Claude';
      default: return part.charAt(0).toUpperCase() + part.slice(1);
    }
  }).join(' ');
}

export function modelReference(reference: string): ModelReference {
  const separator = reference.indexOf('/');
  const qualified = separator > 0 && separator < reference.length - 1;
  const providerId = qualified ? reference.slice(0, separator) : null;
  const requestModel = qualified ? reference.slice(separator + 1) : reference;

  return {
    reference,
    providerId,
    requestModel,
    label: modelLabel(requestModel),
  };
}

export function modelDisplayLabel(
  model: ModelReference,
  details: readonly ModelCatalogDetail[],
): string {
  return details.find((detail) => detail.reference === model.reference)?.display_name?.trim()
    || model.label;
}

export function filterModelGroups(
  groups: readonly ModelProviderGroup[],
  query: string,
  details: readonly ModelCatalogDetail[] = [],
): readonly ModelProviderGroup[] {
  const normalized = query.trim().toLocaleLowerCase();
  if (!normalized) return groups;

  return groups.flatMap((group) => {
    const providerMatches = `${group.providerLabel} ${group.providerId ?? ''}`
      .toLocaleLowerCase()
      .includes(normalized);
    const models = group.models.filter((model) => providerMatches || [
      modelDisplayLabel(model, details),
      model.label,
      model.requestModel,
      model.reference,
    ].some((value) => value.toLocaleLowerCase().includes(normalized)));
    return models.length > 0 ? [{ ...group, models }] : [];
  });
}

export function modelBillingGroups(
  group: ModelProviderGroup,
  details: readonly ModelCatalogDetail[] = [],
): readonly ModelBillingGroup[] {
  if (group.providerId !== 'openrouter') return [{ label: null, models: group.models }];

  const free: ModelReference[] = [];
  const paid: ModelReference[] = [];
  for (const model of group.models) {
    const detail = details.find((candidate) => candidate.reference === model.reference);
    const isFree = detail?.pricing?.billing_mode === 'free'
      || model.requestModel === 'openrouter/free'
      || model.requestModel.endsWith(':free');
    (isFree ? free : paid).push(model);
  }

  return [
    ...(paid.length > 0 ? [{ label: 'Paid' as const, models: paid }] : []),
    ...(free.length > 0 ? [{ label: 'Free' as const, models: free }] : []),
  ];
}

export function groupModelReferences(references: readonly string[]): readonly ModelProviderGroup[] {
  const groups = new Map<string | null, ModelProviderGroup>();
  const seen = new Set<string>();

  for (const reference of references) {
    if (!reference || seen.has(reference)) continue;
    seen.add(reference);

    const model = modelReference(reference);
    const existing = groups.get(model.providerId);
    if (existing) {
      groups.set(model.providerId, { ...existing, models: [...existing.models, model] });
      continue;
    }

    const provider = model.providerId ? providerById(model.providerId) : undefined;
    const providerLabel = model.providerId === 'builtin'
      ? 'Anthropic (Built-in)'
      : provider?.label ?? (model.providerId ? titleCase(model.providerId) : 'Models');
    groups.set(model.providerId, {
      providerId: model.providerId,
      providerLabel,
      models: [model],
    });
  }

  return [...groups.values()];
}

export function providerVisibilityConfig(
  settings: ModelPickerVisibilitySettings | undefined,
  providerId: string,
): ProviderModelPickerVisibility | undefined {
  return settings?.[providerId];
}

export function visibleModelsForProvider(
  providerId: string,
  models: readonly ModelDetailsDto[],
  settings: ModelPickerVisibilitySettings | undefined,
): readonly ModelDetailsDto[] {
  const config = providerVisibilityConfig(settings, providerId);
  if (config?.showInModelPicker === false) return [];
  if (config?.visibleModelIds === undefined) return models;
  if (config.visibleModelIds.length === 0) return [];
  const allowed = new Set(config.visibleModelIds);
  return models.filter((model) => allowed.has(model.model_id));
}

export function selectedModelIdsForPickerSettings(
  candidates: readonly string[],
  visibility: ProviderModelPickerVisibility | undefined,
): string[] {
  const unique = [...new Set(candidates.filter((id) => id.trim().length > 0))];
  if (visibility?.visibleModelIds === undefined) return unique;
  const allowed = new Set(visibility.visibleModelIds);
  return unique.filter((id) => allowed.has(id));
}

export function visibleProviderModelCatalog(
  providers: readonly ProviderModelCatalogEntryDto[],
  settings: ModelPickerVisibilitySettings | undefined,
): readonly VisibleProviderCatalog[] {
  return providers.flatMap((provider) => {
    const models = visibleModelsForProvider(provider.provider_id, provider.models, settings);
    return models.length > 0
      ? [{ providerId: provider.provider_id, displayName: provider.provider_label, models }]
      : [];
  });
}

export function filterVisibleModelReferences(
  references: readonly string[],
  providers: readonly ProviderModelCatalogEntryDto[] | undefined,
  settings: ModelPickerVisibilitySettings | undefined,
): readonly string[] {
  const catalog = providers ?? [];
  if (catalog.length === 0 && (!settings || Object.keys(settings).length === 0)) return references;
  const providerIds = new Set(catalog.map((provider) => provider.provider_id));
  const visible = new Set(
    visibleProviderModelCatalog(catalog, settings).flatMap((provider) => provider.models.map((model) => model.reference)),
  );
  return references.filter((reference) => {
    const parsed = modelReference(reference);
    if (!parsed.providerId) return true;
    if (visible.has(reference)) return true;
    const config = providerVisibilityConfig(settings, parsed.providerId);
    if (!providerIds.has(parsed.providerId)) {
      if (!config) return true;
      if (config.showInModelPicker === false) return false;
      if (config.visibleModelIds === undefined) return true;
      if (config.visibleModelIds.length === 0) return false;
      return config.visibleModelIds.includes(parsed.requestModel);
    }
    return false;
  });
}
