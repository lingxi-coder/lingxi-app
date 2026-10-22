import type { ToolRunItem } from './runItem';

export function localAppReference(item: ToolRunItem): { id: string; name: string; revision: number; summary?: string } | null {
  if (item.tool !== 'LocalApp' || item.status !== 'done') return null;
  let output: unknown = item.nativeOutput;
  if (typeof output === 'string') { try { output = JSON.parse(output); } catch { return null; } }
  if (!output || typeof output !== 'object' || Array.isArray(output)) return null;
  const value = output as Record<string, unknown>;
  return value.action === 'publish' && typeof value.id === 'string' && /^a[a-f0-9]{20}$/.test(value.id) && typeof value.name === 'string' && typeof value.revision === 'number'
    ? { id: value.id, name: value.name, revision: value.revision, summary: typeof value.summary === 'string' ? value.summary.trim() : undefined } : null;
}
