/**
 * The Scope tab of a Bot without the screen. A Bot's scope is the most of the owner's Local Apps and
 * datasets it can be given. What the owner can choose among is what they have on this computer and
 * in their data sources; what the Bot was given that they no longer have is not dropped, because
 * saving the page must not decide that for them: it is shown as gone, with a way to remove it. When the
 * page cannot tell (no account, no connection) it says so and shows what the Bot has as it is.
 */

import type { DataScope, ManagedDataSource, VisibleDataSet } from '../../../../../packages/teams/lingxi-data';
import type { BotDatasetScopeUi, BotScopeUi } from '../../shared/bots';
import type { LingxiApi } from './lingxi';
import { ipcErrorMessage } from './botForm';
import { ObservableState } from './observableState';

/** Where the page learns what the owner has. */
export interface BotScopeSources {
  /** The Local Apps installed on this computer. */
  localApps(): Promise<ReadonlyArray<{ id: string; name: string }>>;
  /** The owner's data sources and what datasets each can reach; absent without an account or a data service. */
  data?: {
    sources(): Promise<readonly ManagedDataSource[]>;
    datasets(scope: DataScope): Promise<readonly VisibleDataSet[]>;
  };
}

/** The sources of the desktop bridge, for the account that is signed in. */
export function desktopScopeSources(host: Pick<LingxiApi, 'localApps' | 'teams'> | undefined, uid: string | undefined): BotScopeSources {
  const teams = host?.teams;
  return {
    localApps: async () => {
      if (!host?.localApps) throw new Error('The Local App host is unavailable.');
      return (await host.localApps.list()).map(({ id, name }) => ({ id, name }));
    },
    ...(uid !== undefined && teams?.localDataSource !== undefined && teams.data !== undefined ? {
      data: {
        sources: async () => {
          const rows = await teams.localDataSource(uid, { action: 'list' });
          if (!Array.isArray(rows)) throw new Error('The data sources could not be read.');
          return rows;
        },
        datasets: async (scope: DataScope) => (await teams.data<{ datasets: VisibleDataSet[] }>(uid, { scope, command: { action: 'datasets' } })).datasets,
      },
    } : {}),
  };
}

export interface ScopeSource {
  source: string;
  name: string;
  kind: string;
  /** What the owner has in the scope of the source; absent when it could not be read. */
  datasets: readonly VisibleDataSet[] | undefined;
}

export interface BotScopeChoices {
  /** Unknown until the first read, and while it failed. */
  apps: ReadonlyArray<{ id: string; name: string }> | undefined;
  appsError: string | undefined;
  /** `unavailable`: there is no account or the data service could not be reached, so nothing is known of the sources. */
  data: 'idle' | 'loading' | 'ready' | 'unavailable';
  sources: readonly ScopeSource[];
}

export class BotScopeChoicesController extends ObservableState<BotScopeChoices> {
  private request = 0;

  /** `sources` is asked each time: the account that is signed in may be another one by then. */
  constructor(private readonly sources: () => BotScopeSources) {
    super({ apps: undefined, appsError: undefined, data: 'idle', sources: [] });
  }

  /** Read what the owner has, again. */
  async load(): Promise<void> {
    const request = ++this.request;
    const where = this.sources();
    this.update({ appsError: undefined, data: where.data === undefined ? 'unavailable' : 'loading', ...(where.data === undefined ? { sources: [] } : {}) });
    await Promise.all([this.loadApps(request, where), this.loadData(request, where)]);
  }

  private async loadApps(request: number, where: BotScopeSources): Promise<void> {
    try {
      const apps = await where.localApps();
      if (request === this.request) this.update({ apps, appsError: undefined });
    } catch (error) {
      if (request === this.request) this.update({ appsError: ipcErrorMessage(error) || 'unavailable' });
    }
  }

  private async loadData(request: number, where: BotScopeSources): Promise<void> {
    const { data } = where;
    if (data === undefined) return;
    try {
      const listed = await data.sources();
      // Sources of one scope share what the scope has: it is read once.
      const reads = new Map<string, Promise<readonly VisibleDataSet[] | undefined>>();
      const sources = await Promise.all(listed.map(async (source): Promise<ScopeSource> => {
        const key = JSON.stringify(source.scope);
        if (!reads.has(key)) reads.set(key, data.datasets(source.scope).catch(() => undefined));
        return { source: source.source, name: source.name, kind: source.kind, datasets: await reads.get(key) };
      }));
      if (request === this.request) this.update({ data: 'ready', sources });
    } catch {
      if (request === this.request) this.update({ data: 'unavailable', sources: [] });
    }
  }
}

export interface ScopeAppRow { id: string; name: string; chosen: boolean }

export interface ScopeDatasetRow {
  dataset: string;
  /** What the owner can do with it; absent when the source could not be read. */
  access: VisibleDataSet['access'] | undefined;
  chosen: boolean;
  write: boolean;
  reportReview: boolean;
  /** The Bot was given it and the source no longer has it. */
  gone: boolean;
}

export interface ScopeSourceGroup {
  source: string;
  name: string;
  kind: string;
  /** Whether what the source has could be read. If not, only what the Bot was given is listed. */
  readable: boolean;
  datasets: ScopeDatasetRow[];
}

export interface ScopeView {
  /** The Local Apps that are installed. */
  apps: ScopeAppRow[];
  /** The Local Apps the Bot was given that are not in `apps`: not installed here (`missing`), or the installed ones could not be read (`unverified`). */
  otherApps: Array<{ id: string; status: 'missing' | 'unverified' }>;
  groups: ScopeSourceGroup[];
  /** Datasets the Bot was given that have no group: their source is gone (`missing`), or the sources could not be read (`unverified`). */
  leftover: Array<BotDatasetScopeUi & { status: 'missing' | 'unverified' }>;
}

const sameDataset = (entry: BotDatasetScopeUi, source: string, dataset: string): boolean => entry.source === source && entry.dataset === dataset;

/** What the page shows: what the owner has with what the Bot was given marked on it, and what the Bot was given that the owner does not have. */
export function scopeView(choices: BotScopeChoices, scope: BotScopeUi): ScopeView {
  const installed = choices.apps;
  const chosenApps = new Set(scope.localApps);
  const apps = (installed ?? []).map((app): ScopeAppRow => ({ id: app.id, name: app.name, chosen: chosenApps.has(app.id) }));
  const otherApps = scope.localApps.filter((id) => !(installed ?? []).some((app) => app.id === id))
    .map((id) => ({ id, status: installed === undefined ? 'unverified' as const : 'missing' as const }));
  if (choices.data !== 'ready') {
    return { apps, otherApps, groups: [], leftover: scope.datasets.map((entry) => ({ ...entry, status: 'unverified' as const })) };
  }
  const groups = choices.sources.map((source): ScopeSourceGroup => {
    const given = scope.datasets.filter((entry) => entry.source === source.source);
    const rows = (source.datasets ?? []).map((listed): ScopeDatasetRow => {
      const entry = given.find((candidate) => candidate.dataset === listed.dataset);
      return { dataset: listed.dataset, access: listed.access, chosen: entry !== undefined, write: entry?.write ?? false, reportReview: entry?.reportReview ?? false, gone: false };
    });
    // What is given and is not listed is gone when the source was read, and unknown when it was not.
    const unlisted = given.filter((entry) => !rows.some((row) => row.dataset === entry.dataset)).map((entry): ScopeDatasetRow => ({
      dataset: entry.dataset, access: undefined, chosen: true, write: entry.write, reportReview: entry.reportReview, gone: source.datasets !== undefined,
    }));
    return { source: source.source, name: source.name, kind: source.kind, readable: source.datasets !== undefined, datasets: [...rows, ...unlisted] };
  });
  const leftover = scope.datasets.filter((entry) => !choices.sources.some((source) => source.source === entry.source)).map((entry) => ({ ...entry, status: 'missing' as const }));
  return { apps, otherApps, groups, leftover };
}

export function withApp(scope: BotScopeUi, appId: string, on: boolean): BotScopeUi {
  const present = scope.localApps.includes(appId);
  if (on) return present ? scope : { ...scope, localApps: [...scope.localApps, appId] };
  return { ...scope, localApps: scope.localApps.filter((id) => id !== appId) };
}

/** Give a dataset, read-only until the owner says more; or take it back. */
export function withDataset(scope: BotScopeUi, source: string, dataset: string, on: boolean): BotScopeUi {
  const present = scope.datasets.some((entry) => sameDataset(entry, source, dataset));
  if (on) return present ? scope : { ...scope, datasets: [...scope.datasets, { source, dataset, write: false, reportReview: false }] };
  return { ...scope, datasets: scope.datasets.filter((entry) => !sameDataset(entry, source, dataset)) };
}

/**
 * Change what the Bot may do with a dataset it was given. Opening report reviews is writing to the dataset
 * (the Bot's host refuses one without the other), so allowing the one allows the other, and stopping the
 * writing stops the reviews.
 */
export function withDatasetAccess(scope: BotScopeUi, source: string, dataset: string, change: Partial<Pick<BotDatasetScopeUi, 'write' | 'reportReview'>>): BotScopeUi {
  return {
    ...scope,
    datasets: scope.datasets.map((entry) => {
      if (!sameDataset(entry, source, dataset)) return entry;
      const next = { ...entry, ...change };
      if (change.reportReview === true) next.write = true;
      if (change.write === false) next.reportReview = false;
      return next;
    }),
  };
}
