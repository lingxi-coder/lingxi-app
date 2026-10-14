import type { FusionCatalogDto, FusionGlobalsDto, FusionScopeCatalogDto, FusionStrategyDto, WritableScopeDto } from '@lingxi/bridge-client';

/** Fusion strategies remain distinct across settings files; scope is never inferred from a merged object. */
export type FusionScope = WritableScopeDto;
export const FUSION_SCOPES: readonly FusionScope[] = ['user', 'project', 'local'];
export const FUSION_SCOPE_LABELS: Record<FusionScope, string> = { user: '用户', project: '项目', local: '本地' };
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
  } catch { return { ...EMPTY_CATALOG, diagnostics: ['无法读取 Fusion 策略目录，请刷新后重试。'] }; }
}
export function fusionStrategyKey(scope: FusionScope, id: string): string { return `${scope}:${id}`; }
export function fusionNeedsLegacyReplacement(scope: FusionCatalogScope | undefined): boolean {
  return Boolean(scope?.diagnostic && scope.strategies.length === 0);
}
export function fusionNameError(name: string, scope: FusionCatalogScope | undefined, id: string): string | null {
  if (!name.trim()) return '请输入策略名称。';
  if (scope?.strategies.some((entry) => entry.id !== id && entry.name.trim() === name.trim())) return '该作用域中已有同名策略，请使用其他名称。';
  return null;
}
export function copiedFusionStrategy(source: FusionStrategyEntry, id: string): FusionStrategyEntry {
  return { id, name: `${source.name} 副本`, config: structuredClone(source.config) };
}
