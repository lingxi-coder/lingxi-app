import type { SettingsSnapshot } from './useEngineSettings';

/**
 * One key's raw value in `layer`'s OWN settings map — never
 * `snapshot.effective` (the cross-layer merge). `update_settings` replaces a
 * key WHOLESALE in one layer's file (`migrations/src/settings_update.rs`),
 * so basing a write on the merged view would fork other layers'
 * contributions into whichever layer gets saved — the bug `CustomProviders`
 * was fixed for in Task 17 fix round 1, and the same reason every reader
 * below goes through `snapshot.layers[layer]` instead.
 *
 * Pulled out of `ToolsAgent.tsx` into its own module (Task 18 fix round 1,
 * Minor) so `Permissions`/`Plugins`/`Skills`/`Hooks` share ONE copy of this
 * gate instead of five pages independently re-deriving the same read.
 */
export function layerValue(snapshot: SettingsSnapshot | null, layer: string, key: string): unknown {
  return snapshot?.layers?.[layer]?.[key];
}

/** Strict: only a literal `true` in that layer counts. A non-boolean value (e.g. a stray string) must never be coerced to `true`. */
export function boolFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): boolean {
  return layerValue(snapshot, layer, key) === true;
}

export function stringFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): string {
  const value = layerValue(snapshot, layer, key);
  return typeof value === 'string' ? value : '';
}

export function stringArrayFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): string[] {
  const value = layerValue(snapshot, layer, key);
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : [];
}

export function stringMapFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): Record<string, string> {
  const value = layerValue(snapshot, layer, key);
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (typeof v === 'string') out[k] = v;
  }
  return out;
}

/** Same gate, for a key whose layer value is an arbitrary JSON object rather than a string map (`enabledPlugins`, `pluginConfigs`, `additionalMarketplaces`, `permissions`, …). */
export function objectFromLayer(snapshot: SettingsSnapshot | null, layer: string, key: string): Record<string, unknown> {
  const value = layerValue(snapshot, layer, key);
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}
