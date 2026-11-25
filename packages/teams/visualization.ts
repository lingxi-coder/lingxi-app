import { isResourceRef, type ResourceRef } from './dataset-resource.js';
import type { TeamsApi } from './index.js';

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

export function parseVisualizationArtifact(value: unknown): VisualizationArtifact | undefined {
  if (!object(value) || typeof value.artifactId !== 'string' || !id.test(value.artifactId)
      || typeof value.name !== 'string' || !Array.isArray(value.parts) || value.parts.length !== 1) return undefined;
  const part = value.parts[0];
  if (!object(part) || part.mediaType !== 'application/json' || !object(part.data) || part.data.version !== 1
      || !object(part.data.bundle) || typeof part.data.bundle.title !== 'string' || value.name !== part.data.bundle.title
      || typeof part.data.bundle.html !== 'string' || !Array.isArray(part.data.bindings) || part.data.bindings.length > 4) return undefined;
  const bindings = part.data.bindings;
  if (bindings.some(entry => !object(entry) || typeof entry.alias !== 'string' || !id.test(entry.alias)
      || typeof entry.messageId !== 'string' || !id.test(entry.messageId)
      || (entry.sourceAlias !== undefined && (typeof entry.sourceAlias !== 'string' || !id.test(entry.sourceAlias))))) return undefined;
  return value as unknown as VisualizationArtifact;
}

export function visualizationLabels(locale: string) {
  const lang = locale.toLowerCase();
  if (lang.startsWith('zh')) return {
    add: '添加可视化', forward: '转发可视化', destination: '目标频道', preview: '预览', title: '标题', import: '导入 HTML', source: 'HTML', send: '发送', close: '关闭',
    cancel: '取消', audience: '分享给', alias: '绑定名', choose: '选择数据', selected: '已选数据', retry: '重试',
    expand: '展开', collapse: '收起', unavailable: '可视化无法打开', confirm: '确认数据更改', change: '提交更改',
    conflict: '记录已更新。请核对当前值后提交。', current: '当前值', next: '新值', field: '字段',
    none: '纯 HTML', mine: '个人数据', team: '团队数据', shared: '频道已共享', fields: '可见字段', partitions: '可见分区',
    pinned: '固定快照', live: '查看最新', edit: '共同编辑', remove: '移除', building: '正在生成固定快照…',
  };
  return {
    add: 'Add visualization', forward: 'Forward visualization', destination: 'Destination channel', preview: 'Preview', title: 'Title', import: 'Import HTML', source: 'HTML', send: 'Send', close: 'Close',
    cancel: 'Cancel', audience: 'Share with', alias: 'Binding name', choose: 'Choose data', selected: 'Selected data', retry: 'Retry',
    expand: 'Expand', collapse: 'Collapse', unavailable: 'Visualization unavailable', confirm: 'Confirm data changes', change: 'Apply changes',
    conflict: 'This record changed. Review the current values before applying.', current: 'Current value', next: 'New value', field: 'Field',
    none: 'HTML only', mine: 'Personal data', team: 'Team data', shared: 'Shared in this channel', fields: 'Visible fields', partitions: 'Visible partitions',
    pinned: 'Fixed snapshot', live: 'View latest', edit: 'Edit together', remove: 'Remove', building: 'Preparing a fixed snapshot…',
  };
}
