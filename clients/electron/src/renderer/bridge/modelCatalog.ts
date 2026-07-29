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
