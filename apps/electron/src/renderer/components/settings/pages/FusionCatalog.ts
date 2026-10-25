import { tr } from '../../../i18n/locale';
import type { FusionCatalogDto, FusionGlobalsDto, FusionScopeCatalogDto, FusionStrategyDto, WritableScopeDto } from '@lingxi/bridge-client';

/** Fusion strategies remain distinct across settings files; scope is never inferred from a merged object. */
export type FusionScope = WritableScopeDto;
export const FUSION_SCOPES: readonly FusionScope[] = ['user', 'project', 'local'];
export const FUSION_SCOPE_LABELS: Record<FusionScope, string> = { get user() { return tr('settings_parity_user_layer'); }, get project() { return tr('settings_parity_project_layer'); }, get local() { return tr('settings_parity_local_layer'); } };
export type FusionStrategyEntry = FusionStrategyDto;
export type FusionGlobals = FusionGlobalsDto;
export type FusionCatalogScope = FusionScopeCatalogDto;
export type FusionCatalog = FusionCatalogDto;
const EMPTY_CATALOG: FusionCatalog = { scopes: [], effective_globals: {}, default_strategy: null, diagnostics: [] };
export function parseFusionCatalog(raw: string | null | undefined): FusionCatalog {
  if (!raw) return EMPTY_CATALOG;
  try {
    const value = JSON.parse(raw) as FusionCatalog;
    if (!Array.isArray(value.scopes) || !value.effective_globals || !Array.isArray(value.diagnostics)) throw new Error('Invalid catalog');
    return value;
  } catch { return { ...EMPTY_CATALOG, diagnostics: [tr('desktop_shell_fusion_catalog_error')] }; }
}
export function fusionStrategyKey(scope: FusionScope, id: string): string { return `${scope}:${id}`; }
export function fusionNameError(name: string, scope: FusionCatalogScope | undefined, id: string): string | null {
  if (!name.trim()) return tr('desktop_shell_fusion_name_required');
  if (scope?.strategies.some((entry) => entry.id !== id && entry.name.trim() === name.trim())) return tr('desktop_shell_fusion_duplicate');
  return null;
}
export function copiedFusionStrategy(source: FusionStrategyEntry, id: string): FusionStrategyEntry {
  return { id, name: tr('desktop_shell_fusion_copy', source.name), config: structuredClone(source.config) };
}
