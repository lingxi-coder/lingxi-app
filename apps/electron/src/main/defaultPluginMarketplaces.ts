import { mkdir, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { ClientEvent, PluginAdminCommandDto } from '@lingxi/bridge-client';
import { decidePublicMarketplaceSourceAction } from '../shared/publicMarketplace.js';
import { claudeCodeOfficialMarketplace } from '../shared/pluginInstallCounts.js';

const defaultMarketplaces = [
  claudeCodeOfficialMarketplace,
  { name: 'anthropic-agent-skills', source: 'anthropics/skills' },
  { name: 'knowledge-work-plugins', source: 'anthropics/knowledge-work-plugins' },
] as const;

interface PluginMarketplaceCatalog {
  marketplaces: { name: string; source_json?: string }[];
  revisions: { user: string };
}

function pluginMarketplaceCatalog(events: readonly ClientEvent[]): PluginMarketplaceCatalog | undefined {
  const event = events.find((item) => item.type === 'plugin_catalog');
  if (!event) return undefined;
  const catalog = JSON.parse(event.catalog_json) as Partial<PluginMarketplaceCatalog> | null;
  return catalog && Array.isArray(catalog.marketplaces) && catalog.revisions?.user
    ? { marketplaces: catalog.marketplaces, revisions: catalog.revisions } : undefined;
}

interface DefaultMarketplaceSeedOptions {
  brandHome: string;
  pluginAdmin: (command: PluginAdminCommandDto) => Promise<ClientEvent[]>;
  onAdded: () => void;
  onError: (source: string, error: unknown) => void;
}

/** Register defaults outside a chat session, after reading the current user revision. */
export async function seedDefaultPluginMarketplaces({ brandHome, pluginAdmin, onAdded, onError }: DefaultMarketplaceSeedOptions) {
  const markerDirectory = join(brandHome, 'plugin-marketplace-defaults');
  await mkdir(markerDirectory, { recursive: true, mode: 0o700 });
  const previouslyRegistered = new Set(await readdir(markerDirectory));
  let catalog: PluginMarketplaceCatalog | undefined;
  for (const marketplace of defaultMarketplaces) {
    const { name, source } = marketplace;
    if (previouslyRegistered.has(name)) continue;
    catalog ??= pluginMarketplaceCatalog(await pluginAdmin({ action: 'get_catalog' }));
    if (!catalog) throw new Error('Plugin marketplace catalog has no user revision');

    const needsRegistration = decidePublicMarketplaceSourceAction(marketplace, catalog.marketplaces).kind === 'add';
    if (needsRegistration) {
      const revision = catalog.revisions.user;
      catalog = undefined;
      try {
        const result = await pluginAdmin({
          action: 'apply_operation', operation_id: 1, scope: 'user', revision,
          payload_json: JSON.stringify({ action: 'marketplace_add', source, scope: 'user', confirmed: true }),
        });
        if (!result.some((event) => event.type === 'configuration_operation' && event.domain === 'plugin' && event.status === 'succeeded')) {
          throw new Error('Plugin marketplace registration was not acknowledged');
        }
        catalog = pluginMarketplaceCatalog(result);
      } catch (error) {
        onError(source, error);
        continue;
      }
    }

    // Leave a marker for successful registrations, existing sources and name
    // conflicts so user removals and different sources survive later launches.
    await writeFile(join(markerDirectory, name), '', { mode: 0o600 });
    if (needsRegistration) onAdded();
  }
}
