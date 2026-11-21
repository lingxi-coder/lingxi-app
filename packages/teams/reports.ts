import type { TeamDataScope } from './lingxi-data.js';

export interface ReportSpec {
  title: string;
  partition: string;
  group: string;
  metric: { kind: 'count' } | { kind: 'sum'; field: string; scale: number };
}
export interface ReportReader { uid: string; partitions: '*' | string[] }
export type ReportOrigin = { kind: 'personal' | 'team'; dataset: string; epoch: number }
  | { kind: 'source'; source: string; version: number };
export interface ReportPlan {
  parent: string | null;
  audienceRevision: string;
  spec: ReportSpec;
  origin: ReportOrigin;
  readers: ReportReader[];
  bots: string[];
}
export interface ReportContext {
  head: string | null;
  audienceRevision: string;
  members: { uid: string; generation: number; access: string }[];
}
export interface ReportCapture { payload: string; signature: string }
export interface ReportDraft { binding: string; confirmation: string; facts: { partition: string; group: string; units: string }[] }
export interface ReportRef { dataset: string; release: string }
export interface PublishedReport extends ReportRef {
  spec: ReportSpec; createdAt: number; canReview: boolean;
  provenance: { kind: ReportOrigin['kind']; version: number; inputDigest?: string; schemaVersion?: number };
  points: { group: string; value: string }[];
  cells: { partition: string; group: string; value: string }[];
}
export type ReportRequest = { scope: TeamDataScope; command:
  | { action: 'reportContext'; dataset: string }
  | { action: 'prepareReport'; dataset: string; plan: ReportPlan }
  | { action: 'publishReport'; dataset: string; plan: ReportPlan; confirmation: string; capture?: ReportCapture }
  | { action: 'getReport'; dataset: string; release: string | null }
  | { action: 'openReportReview'; dataset: string; release: string; partition: string; group: string }
  | { action: 'limitReportReader'; dataset: string; uid: string; generation: number; expectedVersion: number; partitions: '*' | string[] }
};

export function reportLink(ref: ReportRef): string {
  return 'lingxi-data:release/' + ref.dataset + '/' + ref.release;
}
export function parseReportLink(text: string): ReportRef | undefined {
  const result = /^lingxi-data:release\/([A-Za-z0-9_-]{1,64})\/([a-f0-9]{64})$/u.exec(text);
  if (result && result[0] === text) return { dataset: result[1]!, release: result[2]! };
  return undefined;
}
