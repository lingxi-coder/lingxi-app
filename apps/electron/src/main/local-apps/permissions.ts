import type { LocalAppCapability } from '../../shared/localApps.js';

export function isAppUrl(id: string, raw: string): boolean {
  try {
    const url = new URL(raw);
    return url.protocol === 'lingxi-app:' && url.hostname === id && !url.port && !url.username && !url.password;
  } catch { return false; }
}

/** Electron uses a singular mediaType for checks and mediaTypes for requests. */
export function permissionCapabilities(kind: string, details: unknown): LocalAppCapability[] {
  if (kind === 'media') {
    const media = details as { mediaType?: string; mediaTypes?: string[] } | undefined;
    const types = media?.mediaTypes ?? (media?.mediaType && media.mediaType !== 'unknown' ? [media.mediaType] : ['video', 'audio']);
    return types.flatMap(type => type === 'video' ? ['camera' as const] : type === 'audio' ? ['microphone' as const] : []);
  }
  return kind === 'notifications' ? ['notifications'] : kind.startsWith('clipboard') ? ['clipboard'] : [];
}
