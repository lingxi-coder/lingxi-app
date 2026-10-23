import type { ToolRunItem } from '../model/runItem';
import { validBundleId } from '../../shared/applicationIcon';

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}

/** Use the application recorded with the action; never query today's foreground app for history. */
export function computerApplication(item: ToolRunItem): { bundleId: string; name: string } | null {
  if (item.tool.toLowerCase() !== 'computer') return null;
  const output = record(item.nativeOutput);
  const app = record(output?.application);
  if (validBundleId(app?.bundle_id)) return { bundleId: app.bundle_id, name: typeof app.name === 'string' ? app.name : app.bundle_id };
  if (Array.isArray(output?.results)) {
    const apps = output.results.map(step => record(record(step)?.result)?.application).map(record);
    const first = apps[0];
    if (validBundleId(first?.bundle_id) && apps.every(app => app?.bundle_id === first.bundle_id)) {
      return { bundleId: first.bundle_id, name: typeof first.name === 'string' ? first.name : first.bundle_id };
    }
    return null;
  }
  const input = record(item.nativeInput);
  if (input?.action === 'open_application' && validBundleId(input.bundle_id)) {
    return { bundleId: input.bundle_id, name: input.bundle_id };
  }
  return null;
}

export function groupComputerApplication(tools: readonly ToolRunItem[]): ToolRunItem | undefined {
  // A mixed-app group has no single application identity. Its expanded rows retain their own icons.
  const apps = tools.filter(item => computerApplication(item));
  const ids = new Set(apps.map(item => computerApplication(item)!.bundleId));
  return ids.size === 1 ? apps[0] : undefined;
}
