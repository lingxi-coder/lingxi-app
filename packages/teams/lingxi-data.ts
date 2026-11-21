import type { ReportCapture, ReportDraft, ReportPlan, ReportRequest } from './reports.js';

export const LINGXI_DATA_BASELINE = '4905f6ac3bbc997932d5024d7df52c0b5883f481';

export interface DataRef {
  dataset: string;
  record: string;
  revision: number;
  schemaVersion: number;
}

export type DataScope = { kind: 'personal' } | {
  kind: 'team';
  teamId: string;
  conversationId: string;
  generation: number;
};

export type DataRead =
  | { action: 'describe'; dataset: string }
  | { action: 'get'; reference: DataRef }
  | { action: 'latest'; dataset: string; record: string }
  | { action: 'list'; dataset: string; after?: string; limit?: number }
  | { action: 'events'; dataset: string; after: number; waitMs?: number };

export interface DataWrite {
  action: 'put';
  dataset: string;
  record: string;
  expectedRevision: number;
  schemaVersion: number;
  value: Record<string, unknown>;
  archive?: boolean;
}

export type DataRequest = ReportRequest | { scope: DataScope; command: DataRead; operation?: never }
  | { scope: DataScope; command: DataWrite; operation: string }
  | { scope: DataScope; operation: string; command: { action: 'setView'; dataset: string; view: string;
    expectedEpoch: number; definition: { kind: 'table' | 'chart' | 'form'; fields: string[] } | null } }
  | { scope: DataScope; command: { action: 'templateViews'; dataset: string } }
  | { scope: DataScope; command: { action: 'setTemplateView'; dataset: string; view: string; schemaVersion: number;
    expectedVersion: number; definition: DataTemplateSpec | null } }
  | { scope: { kind: 'personal' }; command: { action: 'create'; schema: Record<string, unknown> }; operation: string }
  | { scope: { kind: 'personal' }; command: { action: 'schema'; dataset: string; expectedEpoch: number;
    schema: Record<string, unknown>; migration?: { rename?: Record<string, string>; defaults?: Record<string, unknown>; drop?: string[] } }; operation: string }
  | { scope: DataScope; command: { action: 'datasets' }; operation?: never }
  | { scope: TeamDataScope; command: { action: 'previewAudience'; editors: string[] }; operation?: never }
  | { scope: TeamDataScope; command: { action: 'privateSourceOffers' }; operation?: never }
  | { scope: TeamDataScope; command: { action: 'dataAutomation'; dataset: string }; operation?: never }
  | { action: 'setBotDataAccess'; scope: TeamDataScope; dataset: string; sharedBotId: string;
    expectedVersion: number; access: 'read' | 'edit' | null; reportReview?: true; handoff?: { targets: string[]; dailyLimit: number } }
  | { action: 'setDataRule'; scope: TeamDataScope; dataset: string; rule: string; expectedVersion: number; config: DataRuleConfig | null }
  | { scope: TeamDataScope; command: { action: 'createTeam'; schema: Record<string, unknown>; editors: string[]; audienceRevision: string }; operation: string }
  | { scope: TeamDataScope; command: { action: 'publishPersonal'; dataset: string; schemaVersion: number;
    sourceEpoch: number; records: DataRef[]; templates?: { view: string; version: number }[];
    editors: string[]; audienceRevision: string }; operation: string };

export type TeamDataScope = Extract<DataScope, { kind: 'team' }>;
export type PrivateSourceOffer = { source: string; name: string; sharedBotId: string; botName: string };
export type DataRuleConfig = { sharedBotId: string; dailyLimit: number; definition: { kind: 'created' }
  | { kind: 'transition'; field: string; equals: string | number | boolean | null } };
export interface DataAutomationSettings {
  bots: { sharedBotId: string; name: string; version: number; access: 'read' | 'edit' | null; reportReview?: true; handoff?: { targets: string[]; dailyLimit: number } }[];
  rules: { rule: string; version: number; config: DataRuleConfig | null }[];
}

/** The inputs a LingXi Data host reads, with their product names. */
export const DATA_SOURCE_KINDS = {
  mysql: 'MySQL', postgres: 'PostgreSQL', sqlite: 'SQLite', mongodb: 'MongoDB', redis: 'Redis', csv: 'CSV', json: 'JSON',
} as const;
export type DataSourceKind = keyof typeof DATA_SOURCE_KINDS;

export interface ManagedDataSource {
  source: string; scope: DataScope; name: string; kind: DataSourceKind;
  enabled: boolean; version: number; requesters: string[];
  pending?: true; desiredEnabled?: boolean;
  grants: { sharedBotId: string; fields: string[]; preset: string }[];
}
export type SourceDraft = { source: string; kind: ManagedDataSource['kind']; fields: string[] };
export type SourceConfiguration = Pick<ManagedDataSource, 'source' | 'scope' | 'name' | 'enabled' | 'requesters' | 'grants'> & { expectedVersion: number };
export type LocalDataSourceCommand =
  | { action: 'import' }
  | { action: 'list' }
  | { action: 'configure'; input: SourceConfiguration }
  | { action: 'captureReport'; source: string; dataset: string; scope: TeamDataScope; plan: ReportPlan }
  | { action: 'details' | 'check' | 'start' | 'stop' | 'preview'; source: string };
export type LocalDataSourceAnswer = SourceDraft | (ManagedDataSource & { running: boolean })[] | { running: boolean }
  | { rows: Record<string, unknown>[] } | (ReportDraft & { capture: ReportCapture }) | null;

export interface DataAudience {
  audienceRevision: string;
  audience: { uid: string; generation: number; access: 'read' | 'edit' | 'manage' }[];
}

export interface VisibleDataSet {
  dataset: string;
  access: 'read' | 'edit' | 'manage';
}

export interface DataDescription {
  schemaVersion: number;
  epoch: number;
  schema: Record<string, unknown>;
  rules: Record<string, { field: string; equals: unknown }>;
  views?: Record<string, { kind: 'table' | 'chart' | 'form'; fields: string[] }>;
}
export type DataTemplateTag = 'section' | 'div' | 'h3' | 'p' | 'span' | 'strong' | 'small' | 'ul' | 'li';
export type DataTemplateNode = { kind: 'text'; text: string } | { kind: 'field'; field: string }
  | { kind: 'element'; tag: DataTemplateTag; style?: Record<string, string>; children: DataTemplateNode[] };
export type DataTemplateSpec = { version: 1; nodes: DataTemplateNode[] };
export interface DataTemplateView { view: string; version: number; schemaVersion: number; definition: DataTemplateSpec }

export interface DataEventsPage {
  cursor: number;
  events: { cursor: number; kind: string; reference: DataRef | null }[];
}

export interface DataRecordPage {
  records: { id: string; record: DataRecord }[];
  cursor: string | null;
}

export interface DataRecord {
  revision: number;
  schemaVersion: number;
  value: Record<string, unknown>;
  archived: boolean;
  actor: { id: string; task?: string };
  origin: string;
}

export interface DataRecordResult {
  reference: DataRef;
  schema: Record<string, unknown>;
  record: DataRecord;
}

function resourceId(value: unknown): value is string {
  return typeof value === 'string' && /^[A-Za-z0-9_-]{1,64}$/u.test(value);
}

function version(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0;
}

export function parseDataRef(value: unknown): DataRef | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const { dataset, record, revision, schemaVersion } = value as Record<string, unknown>;
  if (Object.keys(value).length !== 4 || !resourceId(dataset) || !resourceId(record)
    || !version(revision) || !version(schemaVersion)) return undefined;
  return { dataset, record, revision, schemaVersion };
}
