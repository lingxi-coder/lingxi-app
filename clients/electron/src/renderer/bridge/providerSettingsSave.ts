import type { LingxiApi } from './lingxi';

type SettingsHost = Pick<LingxiApi, 'command' | 'onEvent'> & Partial<Pick<LingxiApi, 'onConnectionStateChanged'>>;

function equalJson(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false;
  if (Array.isArray(left) !== Array.isArray(right)) return false;
  const a = Object.keys(left), b = Object.keys(right);
  return a.length === b.length && a.every((key) => Object.prototype.hasOwnProperty.call(right, key)
    && equalJson((left as Record<string, unknown>)[key], (right as Record<string, unknown>)[key]));
}

/** update_settings has no operation ID; confirm the written layer before reporting success. */
export function saveProviderSettings(
  host: SettingsHost,
  sessionId: string,
  destination: 'user' | 'project' | 'local',
  patch: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<void> {
  const patchJson = JSON.stringify(patch);
  const persistedPatch: Record<string, unknown> = JSON.parse(patchJson);
  return new Promise((resolve, reject) => {
    let offEvent = (): void => undefined;
    let offState = (): void => undefined;
    let settled = false;
    const finish = (error?: Error): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      offEvent();
      offState();
      signal?.removeEventListener('abort', abort);
      if (error) reject(error); else resolve();
    };
    const abort = (): void => finish(new Error('Provider settings save was interrupted.'));
    const timer = setTimeout(() => finish(new Error('Timed out confirming provider settings were saved.')), 10_000);
    if (signal?.aborted) { abort(); return; }
    signal?.addEventListener('abort', abort, { once: true });
    offEvent = host.onEvent(({ sessionId: incoming, event }) => {
      if (incoming !== sessionId) return;
      // Without a request ID, fail conservatively on an engine error during this write.
      if (event.type === 'error') { finish(new Error('The engine could not save provider settings.')); return; }
      if (event.type !== 'settings_snapshot' || !event.layers_json) return;
      try {
        const layer = JSON.parse(event.layers_json)[destination];
        if (!layer || typeof layer !== 'object' || Array.isArray(layer)) return;
        if (Object.entries(persistedPatch).every(([key, value]) => value === null
          ? !Object.prototype.hasOwnProperty.call(layer, key) : equalJson(layer[key], value))) finish();
      } catch { /* Malformed snapshots cannot confirm a successful write. */ }
    });
    offState = host.onConnectionStateChanged?.(({ sessionId: incoming, event }) => {
      if (incoming === sessionId && event.status !== 'connected') abort();
    }) ?? (() => undefined);
    void host.command(sessionId, { type: 'update_settings', destination, patch_json: patchJson })
      .catch((cause: unknown) => finish(cause instanceof Error ? cause : new Error(String(cause))));
  });
}
