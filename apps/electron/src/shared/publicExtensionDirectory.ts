import directory from '../../../../resources/extensions/public-directory.json';

export type PublicExtensionPlatform = 'macos' | 'windows' | 'linux' | 'android' | 'ios';
export type PublicExtensionText = Readonly<Record<string, string>>;

export interface PublicExtensionItem {
  id: string;
  kind: 'mcp' | 'plugin';
  name: string;
  publisher: string;
  category: 'browser' | 'development' | 'productivity';
  platforms: readonly PublicExtensionPlatform[];
  description: PublicExtensionText;
  requirements?: PublicExtensionText;
  sourceUrl: string;
  documentationUrl: string;
  version?: string;
  /** Discoverable skills supplied by the original, complete plugin package. */
  skills?: readonly string[];
  examples?: readonly PublicExtensionText[];
  mcp?: { name: string; config: Readonly<Record<string, unknown>> };
  marketplace?: { source: string; name: string; plugin?: string };
}

/** Bundled discovery metadata. Reading the directory never installs or enables anything. */
export const publicExtensionItems: readonly PublicExtensionItem[] = directory.items as PublicExtensionItem[];

export const publicSkillRecommendations = publicExtensionItems.filter((item) => item.skills?.length);

const DIRECTORY_NAVIGATION_PREFIX = 'directory:';

export function publicExtensionNavigationId(item: PublicExtensionItem, action: 'details' | 'add' | 'uninstall' = 'details'): string {
  return `${DIRECTORY_NAVIGATION_PREFIX}${item.id}:${action}`;
}

export function publicExtensionFromNavigation(id: string | undefined): PublicExtensionItem | undefined {
  if (!id?.startsWith(DIRECTORY_NAVIGATION_PREFIX)) return undefined;
  return publicExtensionItems.find((item) => item.id === id.slice(DIRECTORY_NAVIGATION_PREFIX.length).split(':')[0] && item.marketplace);
}

export function publicExtensionNavigationAction(id: string | undefined): 'details' | 'add' | 'uninstall' {
  const action = id?.startsWith(DIRECTORY_NAVIGATION_PREFIX) ? id.slice(DIRECTORY_NAVIGATION_PREFIX.length).split(':')[1] : undefined;
  return action === 'add' || action === 'uninstall' ? action : 'details';
}

export function publicExtensionsForPlatform(platform: PublicExtensionPlatform): readonly PublicExtensionItem[] {
  return publicExtensionItems.filter((item) => item.platforms.includes(platform));
}

/** Callers pass their selected UI language; the catalog always includes English. */
export function publicExtensionText(value: PublicExtensionText | undefined, locale = 'en'): string {
  if (!value) return '';
  const parts = locale.replace(/_/g, '-').toLowerCase().split('-');
  const key = parts[0] === 'zh'
    ? parts.includes('hant') || (!parts.includes('hans') && parts.some((part) => ['tw', 'hk', 'mo'].includes(part))) ? 'zh-Hant' : 'zh-Hans'
    : parts[0];
  return value[key] ?? value.en ?? '';
}
