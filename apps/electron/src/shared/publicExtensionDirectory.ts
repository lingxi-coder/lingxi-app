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
  mcp?: { name: string; config: Readonly<Record<string, unknown>> };
  marketplace?: { source: string; name: string; plugin?: string };
}

/** Bundled discovery metadata. Reading the directory never installs or enables anything. */
export const publicExtensionItems: readonly PublicExtensionItem[] = directory.items as PublicExtensionItem[];

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
