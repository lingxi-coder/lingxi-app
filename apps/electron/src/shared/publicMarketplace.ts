import type { PublicExtensionItem } from './publicExtensionDirectory';

function validGithubSegment(value: string): boolean {
  return /^[a-z0-9_.-]+$/i.test(value) && value !== '.' && value !== '..';
}

function githubShorthandIdentity(source: string): string | null {
  const match = source.trim().match(/^([a-z0-9_.-]+)\/([a-z0-9_.-]+)(?:\.git)?$/i);
  if (!match || !validGithubSegment(match[1]!) || !validGithubSegment(match[2]!)) return null;
  return `${match[1]!.toLowerCase()}/${match[2]!.replace(/\.git$/i, '').toLowerCase()}`;
}

/** Canonicalize only a root GitHub remote. Reject credentials, refs, queries and subpaths. */
function githubRemoteIdentity(source: string): string | null {
  const value = source.trim();
  if (value.includes('?') || value.includes('#')) return null;
  const scp = value.match(/^git@github\.com:([a-z0-9_.-]+)\/([a-z0-9_.-]+)(?:\.git)?$/i);
  if (scp && validGithubSegment(scp[1]!) && validGithubSegment(scp[2]!)) {
    return `${scp[1]!.toLowerCase()}/${scp[2]!.replace(/\.git$/i, '').toLowerCase()}`;
  }
  if (value.includes('\\')) return null;
  const rawRemote = value.match(/^(?:https|ssh):\/\/[^/?#]*(\/[^?#]*)?$/i);
  if (!rawRemote) return null;
  const rawPath = rawRemote[1] ?? '';
  if (rawPath.includes('%')) return null;
  const rawPathSegments = rawPath.replace(/^\/+|\/+$/g, '').split('/').filter(Boolean);
  if (rawPathSegments.some((segment) => segment === '.' || segment === '..')) return null;

  try {
    const url = new URL(value);
    const protocol = url.protocol.toLowerCase();
    if (url.hostname.toLowerCase() !== 'github.com'
      || url.search !== ''
      || url.hash !== '') return null;
    if (protocol === 'https:') {
      if (value.includes('@') || url.username !== '' || url.password !== '' || (url.port !== '' && url.port !== '443')) return null;
    } else if (protocol === 'ssh:') {
      if ((url.username !== '' && url.username !== 'git') || url.password !== '' || (url.port !== '' && url.port !== '22')) return null;
      const authority = value.match(/^ssh:\/\/([^/]+)/i)?.[1] ?? '';
      const userInfo = authority.includes('@') ? authority.slice(0, authority.indexOf('@')) : null;
      if (userInfo !== null && userInfo !== 'git') return null;
    } else {
      return null;
    }
    const path = url.pathname.replace(/^\/+|\/+$/g, '');
    const parts = path.split('/');
    if (parts.length !== 2) return null;
    const owner = parts[0]!;
    const repo = parts[1]!.replace(/\.git$/i, '');
    if (!validGithubSegment(owner) || !validGithubSegment(repo)) return null;
    return `${owner.toLowerCase()}/${repo.toLowerCase()}`;
  } catch {
    return null;
  }
}

/** `marketplace_add` accepts shorthand or a root remote; the stored DTO keeps its discriminator. */
export function githubRepositoryIdentity(source: string): string | null {
  return githubShorthandIdentity(source) ?? githubRemoteIdentity(source);
}

function marketplaceSourceMatches(source: string, sourceJson?: string): boolean {
  if (!sourceJson) return false;
  try {
    const sourceRecord = JSON.parse(sourceJson) as Record<string, unknown>;
    const expectedRepo = githubRepositoryIdentity(source);
    if (!expectedRepo || typeof sourceRecord.source !== 'string') return false;
    const keys = Object.keys(sourceRecord).sort().join(',');
    if (sourceRecord.source === 'github' && keys === 'repo,source' && typeof sourceRecord.repo === 'string') {
      return githubShorthandIdentity(sourceRecord.repo) === expectedRepo;
    }
    if (sourceRecord.source === 'git' && keys === 'source,url' && typeof sourceRecord.url === 'string') {
      return githubRemoteIdentity(sourceRecord.url) === expectedRepo;
    }
    return false;
  } catch {
    return false;
  }
}

export function findRegisteredPublicMarketplace(
  marketplace: PublicExtensionItem['marketplace'],
  registry: readonly { name: string; source_json?: string }[],
) {
  if (!marketplace) return undefined;
  return registry.find((entry) => (
    marketplaceSourceMatches(marketplace.source, entry.source_json)
  ));
}

export function findConflictingPublicMarketplace(
  marketplace: PublicExtensionItem['marketplace'],
  registry: readonly { name: string; source_json?: string }[],
) {
  if (!marketplace) return undefined;
  return registry.find((entry) => (
    entry.name === marketplace.name && !marketplaceSourceMatches(marketplace.source, entry.source_json)
  ));
}

export type PublicMarketplaceSourceAction =
  | { kind: 'registered'; marketplace: { name: string; source_json?: string } }
  | { kind: 'source-conflict'; marketplace: { name: string; source_json?: string } }
  | { kind: 'add' };

export function decidePublicMarketplaceSourceAction(
  marketplace: NonNullable<PublicExtensionItem['marketplace']>,
  registry: readonly { name: string; source_json?: string }[],
): PublicMarketplaceSourceAction {
  const conflict = findConflictingPublicMarketplace(marketplace, registry);
  if (conflict) return { kind: 'source-conflict', marketplace: conflict };
  const registered = findRegisteredPublicMarketplace(marketplace, registry);
  if (registered) return { kind: 'registered', marketplace: registered };
  return { kind: 'add' };
}

export function publicMarketplacePluginTargetId(
  marketplace: PublicExtensionItem['marketplace'],
  sourceAction: PublicMarketplaceSourceAction | null,
): string | null {
  if (!marketplace?.plugin) return null;
  const registeredName = sourceAction?.kind === 'registered' ? sourceAction.marketplace.name : marketplace.name;
  return `${marketplace.plugin}@${registeredName}`;
}

/** A recommendation is installed only when its verified source owns the actual package. */
export function isPublicExtensionInstalled(
  item: PublicExtensionItem,
  catalog: { marketplaces?: readonly { name: string; source_json?: string }[]; installed?: readonly { id: string }[] },
): boolean {
  if (!item.marketplace) return false;
  const source = decidePublicMarketplaceSourceAction(item.marketplace, catalog.marketplaces ?? []);
  const id = publicMarketplacePluginTargetId(item.marketplace, source);
  return source.kind === 'registered' && id !== null && Boolean(catalog.installed?.some((entry) => entry.id === id));
}
