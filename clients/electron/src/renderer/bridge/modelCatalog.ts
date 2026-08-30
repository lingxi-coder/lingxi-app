import { providerById } from '../../shared/providers';

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
