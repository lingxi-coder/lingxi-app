import { z } from 'zod';

/**
 * What a shared Bot may reach of its owner's Desktop in a Team, as ids, names and flags only: never a path, token or secret.
 * Three layers meet at run time and each only narrows: the Bot's own upper bound (`bot.json`, checked by the owner's Desktop when it
 * publishes and again when it runs), the scope the owner published (`published`), and what a Chat narrowed it to (`narrowing`).
 * Datasets then also need a live grant (`lingxi-data-bot-records.mjs`).
 */
const entries = 64;
const appId = z.string().regex(/^[A-Za-z0-9._:-]{1,128}$/u);
// A dataset and the source it is read through have the same shape as everywhere else in the Data service.
const dataId = z.string().regex(/^[A-Za-z0-9_-]{1,64}$/u);
const label = z.string().trim().min(1).max(100).refine(value => !/[\u0000-\u001f\u007f]/u.test(value));
const unique = values => new Set(values).size === values.length;
/** One Local App or dataset appears once. A grant names a dataset by its id alone, so the id is what must not repeat. */
const distinct = (rows, key) => unique(rows.map(row => row[key]));

export const publishedScope = z.object({
  localApps: z.array(z.object({ id: appId, name: label }).strict()).max(entries).refine(rows => distinct(rows, 'id'), 'Duplicate Local App'),
  datasets: z.array(z.object({ source: dataId, dataset: dataId, write: z.boolean(), reportReview: z.boolean() }).strict())
    .max(entries).refine(rows => distinct(rows, 'dataset'), 'Duplicate dataset'),
}).strict();

/** A Chat's narrowing of one Bot: a list that is left out does not narrow that kind of thing. */
export const narrowing = z.object({
  localApps: z.array(appId).max(entries).refine(unique, 'Duplicate Local App').optional(),
  datasets: z.array(z.object({ source: dataId, dataset: dataId }).strict()).max(entries).refine(rows => distinct(rows, 'dataset'), 'Duplicate dataset').optional(),
}).strict();

export const emptyScope = Object.freeze({ localApps: [], datasets: [] });
export const scopeSize = scope => (scope?.localApps.length ?? 0) + (scope?.datasets.length ?? 0);

/** A narrowing asks for no more than the Bot published: every Local App and every (source, dataset) it names is in `published`. */
export function withinPublished(published, wanted) {
  const have = published ?? emptyScope;
  return (wanted.localApps ?? []).every(id => have.localApps.some(app => app.id === id))
    && (wanted.datasets ?? []).every(({ source, dataset }) => have.datasets.some(row => row.source === source && row.dataset === dataset));
}

/** What a Bot may use in a Chat: what it published, cut down to what the Chat allows. A Bot published without a scope has none. */
export function effectiveScope(published, narrowed) {
  const have = published ?? emptyScope;
  return {
    localApps: have.localApps.filter(app => narrowed?.localApps === undefined || narrowed.localApps.includes(app.id)),
    datasets: have.datasets.filter(row => narrowed?.datasets === undefined
      || narrowed.datasets.some(entry => entry.source === row.source && entry.dataset === row.dataset)),
  };
}
