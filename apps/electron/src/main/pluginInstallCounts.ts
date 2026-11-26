import { claudeCodeOfficialMarketplace, type PluginInstallCounts } from '../shared/pluginInstallCounts.js';

const sourceUrl = 'https://raw.githubusercontent.com/anthropics/claude-plugins-official/refs/heads/stats/stats/plugin-installs.json';
const maxBytes = 256 * 1024;
const refreshIntervalMs = 6 * 60 * 60_000;
const retryIntervalMs = 5 * 60_000;

export function parsePluginInstallCounts(data: unknown): PluginInstallCounts {
  const plugins = (data as { plugins?: unknown } | null)?.plugins;
  if (!Array.isArray(plugins)) throw new Error('Invalid Claude Code plugin statistics');

  const entries: Array<[string, number]> = [];
  const suffix = `@${claudeCodeOfficialMarketplace.name}`;
  for (const row of plugins) {
    const { plugin, unique_installs: count } = (row ?? {}) as { plugin?: unknown; unique_installs?: unknown };
    if (typeof plugin !== 'string' || !plugin.endsWith(suffix) || typeof count !== 'number' || !Number.isSafeInteger(count) || count < 0) continue;
    const name = plugin.slice(0, -suffix.length);
    if (name && !name.includes('@')) entries.push([name, count]);
  }
  if (!entries.length) throw new Error('Claude Code plugin statistics contain no valid counts');
  return Object.fromEntries(entries);
}

async function fetchPluginInstallCounts(fetcher: typeof fetch): Promise<PluginInstallCounts> {
  const response = await fetcher(sourceUrl, { credentials: 'omit', redirect: 'error', signal: AbortSignal.timeout(8_000) });
  if (!response.ok || !response.body) throw new Error('Claude Code plugin statistics are unavailable');
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > maxBytes) throw new Error('Claude Code plugin statistics exceed the size limit');
      chunks.push(value);
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
  return parsePluginInstallCounts(JSON.parse(Buffer.concat(chunks).toString('utf8')));
}

export function createPluginInstallCountsReader(fetcher: typeof fetch = fetch) {
  let cached: PluginInstallCounts = {};
  let refreshAfter = 0;
  let inFlight: Promise<PluginInstallCounts> | undefined;
  return () => {
    if (inFlight) return inFlight;
    if (Date.now() < refreshAfter) return Promise.resolve(cached);
    inFlight = fetchPluginInstallCounts(fetcher).then(counts => {
      cached = counts;
      refreshAfter = Date.now() + refreshIntervalMs;
      return cached;
    }, () => {
      refreshAfter = Date.now() + retryIntervalMs;
      return cached;
    }).finally(() => { inFlight = undefined; });
    return inFlight;
  };
}

export const readPluginInstallCounts = createPluginInstallCountsReader();
