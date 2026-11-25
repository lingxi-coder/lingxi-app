/**
 * What a shared Bot may use of its maintainer's Desktop in a Team, as ids, names and flags only: never a path, token or secret.
 * Each layer only narrows the one before it: the Bot's own upper bound (`bot.json` on its Desktop), what the maintainer published,
 * and what a Chat narrows it to. A dataset also needs a live grant for the Chat. `effectiveBotScope` is the middle two, which is
 * what a member is shown as "what this Bot can use here".
 */
export interface PublishedBotScope {
  localApps: Array<{ id: string; name: string }>;
  datasets: Array<{ source: string; dataset: string; write: boolean; reportReview: boolean }>;
}
/** A Chat's narrowing of one Bot: a list that is left out does not narrow that kind of thing. */
export interface BotScopeNarrowing {
  localApps?: string[];
  datasets?: Array<{ source: string; dataset: string }>;
}
/** The narrowing as the Chat holds it, with the revision of this Bot's entry (written back as `expectedRevision`). A lifted narrowing keeps only its revision. */
export type BotScopeEntry = BotScopeNarrowing & { revision: number };

/** The most Local Apps and the most datasets one scope names. */
export const BOT_SCOPE_ENTRIES = 64;
export const emptyBotScope = (): PublishedBotScope => ({ localApps: [], datasets: [] });

/** What the Bot may use in a Chat: what it published, cut down to what the Chat allows. A Bot published without a scope has none. */
export function effectiveBotScope(published: PublishedBotScope | undefined, narrowing: BotScopeNarrowing | undefined): PublishedBotScope {
  const have = published ?? emptyBotScope();
  return {
    localApps: have.localApps.filter(app => narrowing?.localApps === undefined || narrowing.localApps.includes(app.id)),
    datasets: have.datasets.filter(row => narrowing?.datasets === undefined
      || narrowing.datasets.some(entry => entry.source === row.source && entry.dataset === row.dataset)),
  };
}

const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === 'string';
/** The narrowings of a Chat's Bots as its document or the service's projection carries them. An entry that is not well formed is left out, as the Chat then has none for that Bot. */
export function decodeBotScopes(value: unknown): Record<string, BotScopeEntry> | undefined {
  if (!record(value)) return undefined;
  const entries: Record<string, BotScopeEntry> = {};
  for (const [sharedBotId, entry] of Object.entries(value)) {
    if (!record(entry) || !Number.isSafeInteger(entry.revision) || (entry.revision as number) < 1) continue;
    const apps = entry.localApps, rows = entry.datasets;
    if (apps !== undefined && !(Array.isArray(apps) && apps.every(text))) continue;
    if (rows !== undefined && !(Array.isArray(rows) && rows.every(row => record(row) && text(row.source) && text(row.dataset)))) continue;
    entries[sharedBotId] = { revision: entry.revision as number,
      ...(apps === undefined ? {} : { localApps: apps as string[] }),
      ...(rows === undefined ? {} : { datasets: (rows as Array<{ source: string; dataset: string }>).map(({ source, dataset }) => ({ source, dataset })) }) };
  }
  return Object.keys(entries).length ? entries : undefined;
}
/** Whether two decoded sets of narrowings say the same. */
export function sameBotScopes(a: Record<string, BotScopeEntry> | undefined, b: Record<string, BotScopeEntry> | undefined): boolean {
  const left = a ?? {}, right = b ?? {}, keys = Object.keys(left);
  return keys.length === Object.keys(right).length && keys.every(key => right[key] !== undefined && JSON.stringify(left[key]) === JSON.stringify(right[key]));
}
