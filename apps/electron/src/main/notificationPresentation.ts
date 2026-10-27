import { basename } from 'node:path';
import type { SessionRef } from '../shared/settings.js';

/** Keep native banners scannable without exposing a workspace's full path. */
function notificationLine(value: string, limit: number): string {
  const characters = Array.from(value.replace(/\s+/gu, ' ').trim());
  return characters.length > limit ? `${characters.slice(0, limit - 1).join('')}…` : characters.join('');
}

/** `label` names where the session lives when the folder's name would not say it, as for a Bot. */
export function notificationPresentation(title: string, body: string, ref?: SessionRef, platform = process.platform, label?: string) {
  const project = ref ? notificationLine(label ?? basename(ref.projectPath), 60) : '';
  return {
    title: notificationLine(title, 80) || 'LingXi Code',
    body: notificationLine(body, 240),
    ...(platform === 'darwin' && project ? { subtitle: project } : {}),
  };
}
