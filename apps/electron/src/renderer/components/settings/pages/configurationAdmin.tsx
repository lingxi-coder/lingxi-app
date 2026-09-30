import { useMemo } from 'react';
import type { ReactNode } from 'react';
import type { useT } from '../../../theme/ThemeContext';
import { ghostButtonStyle, inputStyle } from './ghostButton';
import type { ConfigurationDomainDto, ConfigurationEffectDto, ConfigurationOperationDto } from '@lingxi/bridge-client';

type Tokens = ReturnType<typeof useT>;

export type JsonRecord = Record<string, unknown>;

let nextOperationIdSeed = Date.now() % 1_000_000;

export function nextConfigurationOperationId(): number {
  nextOperationIdSeed = (nextOperationIdSeed + 1) % 1_000_000;
  return Date.now() * 1_000 + nextOperationIdSeed;
}

export function parseJsonRecord(text: string, label = 'JSON'): { value: JsonRecord } | { error: string } {
  try {
    const parsed = JSON.parse(text) as unknown;
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return { error: `${label} 必须是一个 JSON 对象。` };
    }
    return { value: parsed as JsonRecord };
  } catch (cause) {
    return { error: cause instanceof Error ? `${label} 不是合法的 JSON：${cause.message}` : `${label} 不是合法的 JSON。` };
  }
}

export function prettyJson(value: unknown): string {
  return JSON.stringify(value ?? {}, null, 2);
}

export function asRecord(value: unknown): JsonRecord {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as JsonRecord : {};
}

export function asString(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback;
}

export function asBoolean(value: unknown, fallback = false): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

export function asNumber(value: unknown, fallback = 0): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

export function asStringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : [];
}

export function asRecordArray(value: unknown): JsonRecord[] {
  return Array.isArray(value)
    ? value.filter((entry): entry is JsonRecord => Boolean(entry) && typeof entry === 'object' && !Array.isArray(entry))
    : [];
}

export function keyValueTextToRecord(text: string): JsonRecord {
  return text.split('\n').map((line) => line.trim()).filter(Boolean).reduce<JsonRecord>((out, line) => {
    const pivot = line.indexOf('=');
    if (pivot === -1) out[line] = '';
    else out[line.slice(0, pivot).trim()] = line.slice(pivot + 1).trim();
    return out;
  }, {});
}

export function recordToKeyValueText(value: unknown): string {
  return Object.entries(asRecord(value)).map(([key, item]) => `${key}=${asString(item)}`).join('\n');
}

export function linesToStringArray(text: string): string[] {
  return text.split('\n').map((entry) => entry.trim()).filter(Boolean);
}

export function stringArrayToLines(values: readonly string[]): string {
  return values.join('\n');
}

export function useParsedJsonRecord(text: string, label = 'JSON') {
  return useMemo(() => parseJsonRecord(text, label), [label, text]);
}

export function parseEventEnvelope<T>(raw: string | null | undefined, fallback: T): T {
  if (!raw) return fallback;
  try { return JSON.parse(raw) as T; } catch { return fallback; }
}

export async function sha256Hex(text: string): Promise<string> {
  const bytes = new TextEncoder().encode(text);
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

export function adminRecordCommand(
  action: string,
  options: {
    operation_id?: number;
    target?: string;
    scope?: string;
    revision?: string;
    payload_json?: string;
  } = {},
) {
  const command: {
    action: string;
    operation_id?: number;
    target?: string;
    scope?: string;
    revision?: string;
    payload_json?: string;
  } = { action };
  if (options.operation_id !== undefined) command.operation_id = options.operation_id;
  if (options.target !== undefined) command.target = options.target;
  if (options.scope !== undefined) command.scope = options.scope;
  if (options.revision !== undefined) command.revision = options.revision;
  if (options.payload_json !== undefined) command.payload_json = options.payload_json;
  return command;
}

export function domainOperation(operations: Partial<Record<ConfigurationDomainDto, ConfigurationOperationDto>>, domain: ConfigurationDomainDto) {
  return operations[domain] ?? null;
}

export function operationTone(effect: ConfigurationEffectDto): { color: string; label: string } {
  if (effect === 'applied') return { color: '#0f9d58', label: '已应用' };
  if (effect === 'restart_required') return { color: '#ff9800', label: '需重启' };
  return { color: '#757575', label: '仅校验' };
}

export function managerShellStyle(t: Tokens) {
  return {
    display: 'grid',
    gridTemplateColumns: '280px minmax(0, 1fr)',
    minHeight: 520,
    background: t.surface,
  } as const;
}

export function managerSidebarStyle(t: Tokens) {
  return {
    borderRight: `0.5px solid ${t.border}`,
    padding: 14,
    display: 'grid',
    gridTemplateRows: 'auto minmax(0, 1fr)',
    gap: 12,
    minWidth: 0,
  } as const;
}

export function managerDetailStyle() {
  return {
    padding: 18,
    display: 'grid',
    gap: 14,
    alignContent: 'start',
    minWidth: 0,
  } as const;
}

export function searchInputStyle(t: Tokens) {
  return { ...inputStyle(t), width: '100%' } as const;
}

export function sidebarListStyle() {
  return { display: 'grid', gap: 8, alignContent: 'start', minWidth: 0 } as const;
}

export function sidebarSectionTitleStyle(t: Tokens) {
  return { fontSize: 11, fontWeight: 700, letterSpacing: 0.4, color: t.text4, textTransform: 'uppercase', margin: '4px 0 2px' } as const;
}

export function sidebarButtonStyle(t: Tokens, active: boolean) {
  return {
    width: '100%',
    textAlign: 'left',
    borderRadius: 10,
    border: `0.5px solid ${active ? t.accent : t.border}`,
    background: active ? t.surfaceHover : t.surface,
    color: t.text,
    padding: '10px 12px',
    cursor: 'pointer',
    display: 'grid',
    gap: 3,
  } as const;
}

export function pillStyle(t: Tokens, tone: 'neutral' | 'warn' | 'danger' | 'success' = 'neutral') {
  const colors = {
    neutral: t.text3,
    warn: t.warn,
    danger: t.danger,
    success: t.accent ?? '#0f9d58',
  };
  return {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    borderRadius: 999,
    padding: '2px 8px',
    fontSize: 11,
    fontWeight: 600,
    background: t.surfaceHover,
    color: colors[tone],
  } as const;
}

export function detailGridStyle(columns = 'repeat(2, minmax(0, 1fr))') {
  return { display: 'grid', gridTemplateColumns: columns, gap: 10, minWidth: 0 } as const;
}

export function fieldStackStyle() {
  return { display: 'grid', gap: 6, minWidth: 0 } as const;
}

export function labelStyle(t: Tokens) {
  return { fontSize: 12, fontWeight: 600, color: t.text2 } as const;
}

export function textareaStyle(t: Tokens, rows = 8) {
  return { ...inputStyle(t), width: '100%', minHeight: rows * 22, resize: 'vertical', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace' } as const;
}

export function noteStyle(t: Tokens, tone: 'neutral' | 'warn' | 'danger' = 'neutral') {
  const color = tone === 'warn' ? t.warn : tone === 'danger' ? t.danger : t.text3;
  return {
    borderRadius: 10,
    border: `0.5px solid ${tone === 'danger' ? t.danger : tone === 'warn' ? t.warn : t.border}`,
    background: t.surfaceHover,
    color,
    padding: '10px 12px',
    fontSize: 12.5,
    lineHeight: 1.6,
  } as const;
}

export function actionRowStyle() {
  return { display: 'flex', flexWrap: 'wrap', gap: 8, alignItems: 'center' } as const;
}

export function dangerGhostButtonStyle(t: Tokens, disabled = false) {
  return ghostButtonStyle(t, disabled, true);
}

export function secondaryMetaStyle(t: Tokens) {
  return { fontSize: 12, color: t.text4, lineHeight: 1.5 } as const;
}

export function EmptyDetail({ t, title, body }: { t: Tokens; title: string; body: string }) {
  return (
    <div style={{ ...noteStyle(t), minHeight: 200, display: 'grid', placeItems: 'center', textAlign: 'center' }}>
      <div style={{ maxWidth: 420 }}>
        <div style={{ fontSize: 13.5, fontWeight: 600, color: t.text2, marginBottom: 6 }}>{title}</div>
        <div>{body}</div>
      </div>
    </div>
  );
}

export function DomainOperationBanner({
  t,
  operation,
  fallbackDomainLabel,
}: {
  t: Tokens;
  operation: (ConfigurationOperationDto & { domain?: ConfigurationDomainDto }) | null | undefined;
  fallbackDomainLabel: string;
}) {
  if (!operation) return null;
  const tone = operation.status === 'failed' ? 'danger' : operation.effect === 'restart_required' ? 'warn' : 'neutral';
  const effect = operationTone(operation.effect);
  return (
    <div role={operation.status === 'failed' ? 'alert' : 'status'} style={noteStyle(t, tone)}>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, alignItems: 'center', marginBottom: 4 }}>
        <span style={{ fontWeight: 700 }}>{fallbackDomainLabel}</span>
        <span style={pillStyle(t, operation.status === 'failed' ? 'danger' : operation.status === 'succeeded' ? 'success' : 'neutral')}>
          {operation.status}
        </span>
        <span style={{ ...pillStyle(t, operation.effect === 'restart_required' ? 'warn' : operation.effect === 'applied' ? 'success' : 'neutral'), color: effect.color }}>
          {effect.label}
        </span>
      </div>
      <div>{operation.message ?? '配置操作已更新。'}</div>
    </div>
  );
}

export function SourcePill({
  t,
  label,
  tone = 'neutral',
}: {
  t: Tokens;
  label: string;
  tone?: 'neutral' | 'warn' | 'danger' | 'success';
}) {
  return <span style={pillStyle(t, tone)}>{label}</span>;
}

export function Field({
  t,
  label,
  children,
}: {
  t: Tokens;
  label: string;
  children: ReactNode;
}) {
  return (
    <label style={fieldStackStyle()}>
      <span style={labelStyle(t)}>{label}</span>
      {children}
    </label>
  );
}
