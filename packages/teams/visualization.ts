import { isResourceRef, type ResourceRef } from './dataset-resource.js';
import type { TeamsApi } from './index.js';
import { teamsText } from './i18n.js';

type Page = Extract<ResourceRef, { kind: 'pageVersion' }>;
export type VisualizationDataRef = Exclude<ResourceRef, Page>;
export type VisualizationMode = 'pinned' | 'live' | 'edit';
export interface VisualizationBinding {
  alias: string;
  reference: VisualizationDataRef;
  grant: string;
  mode: VisualizationMode;
}
export interface TeamVisualization {
  reference: Page;
  title: string;
  bindings: VisualizationBinding[];
}
export interface VisualizationBundle {
  title: string;
  html: string;
  assets?: Record<string, { mime: string; data: string }>;
}
export type VisualizationSource =
  | (VisualizationBinding & { kind: 'reuse' })
  | { kind: 'share'; alias: string; dataset: string; via?: string; mode: VisualizationMode;
    reference?: VisualizationDataRef; projection?: { fields?: string[]; partitions?: string[] } };
export interface VisualizationScope {
  teamId: string;
  conversationId: string;
  generation: number;
}
export type VisualizationAudience = { audience: Record<string, number>; audienceDigest: string };

const id = /^[A-Za-z0-9_-]{1,64}$/u;
const validMode = (value: unknown): value is VisualizationMode => value === 'pinned' || value === 'live' || value === 'edit';
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const fields = (value: Record<string, unknown>, ...allowed: string[]) => Object.keys(value).every(key => allowed.includes(key));

export function parseVisualization(value: unknown, messageId: string): TeamVisualization | undefined {
  if (!object(value) || !isResourceRef(value.reference) || value.reference.kind !== 'pageVersion'
      || value.reference.page !== messageId || !messageId.startsWith('viz_')
      || typeof value.title !== 'string' || !value.title.trim() || value.title.length > 80
      || !Array.isArray(value.bindings) || value.bindings.length > 4) return undefined;
  const bindings: VisualizationBinding[] = [];
  for (const entry of value.bindings) {
    if (!object(entry) || typeof entry.alias !== 'string' || !id.test(entry.alias)
        || typeof entry.grant !== 'string' || !id.test(entry.grant) || !isResourceRef(entry.reference)
        || entry.reference.kind === 'pageVersion' || !validMode(entry.mode)
        || bindings.some(existing => existing.alias === entry.alias)) return undefined;
    bindings.push({ alias: entry.alias, reference: entry.reference, grant: entry.grant, mode: entry.mode });
  }
  return { reference: value.reference, title: value.title, bindings };
}

export function visualizationRequest<T>(api: Pick<TeamsApi, 'datasets'>, scope: VisualizationScope, body: Record<string, unknown>): Promise<T> {
  if (!api.datasets) return Promise.reject(new Error('VISUALIZATION_UNAVAILABLE'));
  return api.datasets<T>({ action: 'teamVisualization', request: { ...body, ...scope } });
}

export type VisualizationArtifact = {
  artifactId: string;
  name: string;
  parts: [{ mediaType: 'application/json'; data: { version: 1; bundle: VisualizationBundle;
    bindings: { alias: string; messageId: string; sourceAlias?: string }[] } }];
};

export function parseVisualizationArtifact(value: unknown, taskId?: string): VisualizationArtifact | undefined {
  if (!object(value) || typeof value.artifactId !== 'string' || !id.test(value.artifactId)
      || (taskId !== undefined && value.artifactId !== taskId) || typeof value.name !== 'string'
      || !value.name.trim() || value.name !== value.name.trim() || value.name.length > 80
      || !fields(value, 'artifactId', 'name', 'parts') || !Array.isArray(value.parts) || value.parts.length !== 1) return undefined;
  const part = value.parts[0];
  if (!object(part) || part.mediaType !== 'application/json' || !fields(part, 'mediaType', 'data')
      || !object(part.data) || part.data.version !== 1 || !fields(part.data, 'version', 'bundle', 'bindings')) return undefined;
  const { bundle, bindings } = part.data;
  const encoder = new TextEncoder();
  if (!object(bundle) || value.name !== bundle.title || typeof bundle.html !== 'string' || !bundle.html.trim()
      || encoder.encode(bundle.html).byteLength > 48 * 1024 || !fields(bundle, 'title', 'html', 'assets')
      || !Array.isArray(bindings) || bindings.length > 4) return undefined;
  if (bundle.assets !== undefined) {
    const mimes = ['image/png', 'image/jpeg', 'image/webp', 'text/css', 'text/javascript'];
    if (!object(bundle.assets) || Object.keys(bundle.assets).length > 32
        || Object.entries(bundle.assets).some(([key, asset]) => !/^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,63}$/u.test(key)
          || !object(asset) || typeof asset.mime !== 'string' || !mimes.includes(asset.mime)
          || typeof asset.data !== 'string' || !/^[A-Za-z0-9+/]*={0,2}$/u.test(asset.data) || !fields(asset, 'mime', 'data'))
        || encoder.encode(JSON.stringify(bundle)).byteLength > 2 * 1024 * 1024) return undefined;
  }
  if (bindings.some(entry => !object(entry) || typeof entry.alias !== 'string' || !id.test(entry.alias)
      || typeof entry.messageId !== 'string' || !id.test(entry.messageId)
      || !fields(entry, 'alias', 'messageId', 'sourceAlias')
      || (entry.sourceAlias !== undefined && (typeof entry.sourceAlias !== 'string' || !id.test(entry.sourceAlias))))) return undefined;
  if (new Set(bindings.map(entry => entry.alias)).size !== bindings.length) return undefined;
  return value as unknown as VisualizationArtifact;
}

export function visualizationLabels(locale: string) {
  const t = teamsText(locale);
  return {
    add: t('teams_visualization_add'), forward: t('teams_visualization_forward'), destination: t('teams_visualization_destination'),
    preview: t('teams_visualization_preview'), title: t('teams_visualization_title'), import: t('teams_visualization_import'),
    source: 'HTML', send: t('teams_visualization_send'), close: t('common_close'), cancel: t('common_cancel'),
    audience: (names: string) => t('tw_dataset_audience_fmt', names), alias: t('teams_visualization_alias'),
    choose: t('tw_dataset_add'), selected: t('teams_visualization_source'), retry: t('teams_visualization_retry'),
    expand: t('teams_visualization_expand'), collapse: t('teams_visualization_collapse'), unavailable: t('tw_visualization_unavailable'),
    confirm: t('teams_visualization_confirm'), change: t('teams_visualization_apply'), conflict: t('teams_visualization_conflict'),
    current: t('tw_visualization_current'), next: t('tw_visualization_next'), field: t('tw_visualization_field'),
    none: t('tw_visualization_html_only'), mine: t('teams_visualization_personal'), team: t('teams_visualization_team'),
    shared: t('teams_visualization_shared'), fields: t('teams_visualization_fields'), partitions: t('teams_visualization_partitions'),
    pinned: t('tw_dataset_fixed'), live: t('tw_dataset_live'), edit: t('teams_visualization_edit'),
    remove: t('teams_visualization_remove'), building: t('teams_visualization_building'), more: t('tw_dataset_more'),
  };
}

const visualScopes = new WeakMap<Pick<TeamsApi, 'datasets'>, Map<string, (card: CardObserver) => () => void>>();
const authErrors = new Set(['DATA_ACCESS_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED', 'DATA_NOT_FOUND']);
export const visualizationAccessLost = (error: unknown) => error instanceof Error && authErrors.has(error.message);
type CardObserver = { message: string; version: string; close(error: string): void };

/** One membership check per group of visible cards, with no requests while the tab is hidden. */
export function observeVisualization(api: Pick<TeamsApi, 'datasets'>, scope: VisualizationScope, card: CardObserver) {
  const scopes = visualScopes.get(api) ?? new Map();
  visualScopes.set(api, scopes);
  const key = JSON.stringify([scope.teamId, scope.conversationId, scope.generation]);
  const existing = scopes.get(key);
  if (existing) return existing(card);
  const observers = new Set<CardObserver>();
  let running = false;
  async function refresh() {
    if (running || document.hidden) return;
    running = true;
    const all = [...observers];
    try {
      for (let start = 0; observers.size && start < all.length; start += 40) {
        const group = all.slice(start, start + 40).filter(item => observers.has(item));
        if (!group.length) continue;
        try {
          const { active } = await visualizationRequest<{ active: boolean[] }>(api, scope, {
            action: 'checkBatch', cards: group.map(({ message, version }) => ({ message, version })),
          });
          group.forEach((item, index) => {
            if (active[index] === false && observers.has(item)) item.close('DATA_ACCESS_REQUIRED');
          });
        } catch (failure) {
          if (visualizationAccessLost(failure)) group.forEach(item => {
            if (observers.has(item)) item.close((failure as Error).message);
          });
        }
      }
    } finally { running = false; }
  }
  const timer = window.setInterval(() => void refresh(), 12000);
  const visibility = () => { if (!document.hidden) void refresh(); };
  document.addEventListener('visibilitychange', visibility);
  function add(observer: CardObserver) {
    observers.add(observer);
    return () => {
      observers.delete(observer);
      if (observers.size) return;
      scopes.delete(key);
      document.removeEventListener('visibilitychange', visibility);
      window.clearInterval(timer);
    };
  }
  scopes.set(key, add);
  return add(card);
}
