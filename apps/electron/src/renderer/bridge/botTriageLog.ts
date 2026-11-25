/**
 * The auto-reply log of a Bot without the screen. A Bot's host judges every message nobody asked the Bot about and tells the service what it
 * decided and why, in codes. The Bot's maintainer reads that here: one list for each Team the Bot is published to, newest first, a page at a
 * time. The window never holds a token: Desktop's main process reads the pages and the window gets them as they are.
 */

import type { TriageLogItem, TriageLogPage, TriageLogRequest, TriageTarget } from '../../../../../packages/teams/execution';
import { TRIAGE_LOG_PAGE } from '../../../../../packages/teams/triage-log';
import type { LingxiApi } from './lingxi';
import { ipcErrorMessage } from './botForm';
import { ObservableState } from './observableState';

// What a judged message is called, and the percent a probability is shown as, are the Teams views' too: one definition of each, in packages/teams.
export { TRIAGE_LOG_PAGE, triageOutcome, triagePercent, type TriageOutcome } from '../../../../../packages/teams/triage-log';

/** Where the page learns what the Bot decided. */
export interface TriageLogSources {
  /** The Teams the Bot is published to from this computer. */
  targets(): Promise<TriageTarget[]>;
  page(request: TriageLogRequest): Promise<TriageLogPage>;
}

/** The sources of the desktop bridge, for the account that is signed in; none without an account or the Teams bridge. */
export function desktopTriageSources(host: Pick<LingxiApi, 'teams'> | undefined, uid: string | undefined, botId: string): TriageLogSources | undefined {
  const teams = host?.teams;
  if (uid === undefined || teams?.triageTargets === undefined || teams.triageLog === undefined) return undefined;
  return {
    targets: () => teams.triageTargets(uid, botId),
    page: (request) => teams.triageLog(uid, request),
  };
}

export interface TriageLogSection {
  target: TriageTarget;
  /** Newest first. */
  items: readonly TriageLogItem[];
  /** Pass it back to read the page after the last one; absent at the end of the log. */
  nextCursor: string | undefined;
  /** `loading`: the first page is being read; `more`: the next one is; `failed`: the last read failed (`error` says how), and the rows are what was read before. */
  status: 'loading' | 'ready' | 'more' | 'failed';
  error: string | undefined;
}

export interface TriageLogState {
  /** `unavailable`: no account is signed in, so there is no one to ask for the log. `failed`: the Teams could not be read. */
  phase: 'idle' | 'loading' | 'ready' | 'unavailable' | 'failed';
  error: string | undefined;
  sections: readonly TriageLogSection[];
}

const sectionKey = (target: Pick<TriageTarget, 'teamId' | 'sharedBotId'>) => `${target.teamId}\u0000${target.sharedBotId}`;

export class BotTriageLogController extends ObservableState<TriageLogState> {
  /** Told apart so that a read that was overtaken by a newer one does not change what the newer one showed. */
  private generation = 0;

  /** `sources` is asked each time: the account that is signed in may be another one by then. */
  constructor(private readonly sources: () => TriageLogSources | undefined) {
    super({ phase: 'idle', error: undefined, sections: [] });
  }

  /** Read the Teams the Bot is published to, and the first page of each. Also what "refresh" does. */
  async load(): Promise<void> {
    const generation = ++this.generation;
    const where = this.sources();
    if (where === undefined) {
      this.set({ phase: 'unavailable', error: undefined, sections: [] });
      return;
    }
    this.update({ phase: 'loading', error: undefined });
    let targets: TriageTarget[];
    try {
      targets = await where.targets();
    } catch (error) {
      if (generation === this.generation) this.set({ phase: 'failed', error: ipcErrorMessage(error) || 'unavailable', sections: [] });
      return;
    }
    if (generation !== this.generation) return;
    this.set({ phase: 'ready', error: undefined, sections: targets.map((target) => ({ target, items: [], nextCursor: undefined, status: 'loading', error: undefined })) });
    await Promise.all(targets.map((target) => this.read(generation, where, target, undefined)));
  }

  /** Read the page after the last one of a Team's log. */
  async more(target: Pick<TriageTarget, 'teamId' | 'sharedBotId'>): Promise<void> {
    const where = this.sources();
    const section = this.getSnapshot().sections.find((candidate) => sectionKey(candidate.target) === sectionKey(target));
    if (where === undefined || section === undefined || section.nextCursor === undefined || section.status === 'loading' || section.status === 'more') return;
    await this.read(this.generation, where, section.target, section.nextCursor);
  }

  private async read(generation: number, where: TriageLogSources, target: TriageTarget, cursor: string | undefined): Promise<void> {
    this.patch(target, { status: cursor === undefined ? 'loading' : 'more', error: undefined });
    try {
      const page = await where.page({ teamId: target.teamId, sharedBotId: target.sharedBotId, limit: TRIAGE_LOG_PAGE, ...(cursor !== undefined ? { cursor } : {}) });
      if (generation !== this.generation) return;
      const items = (section: TriageLogSection) => cursor === undefined ? page.items : [...section.items, ...page.items.filter((item) => !section.items.some((known) => known.taskId === item.taskId))];
      this.patch(target, (section) => ({ items: items(section), nextCursor: page.nextCursor, status: 'ready', error: undefined }));
    } catch (error) {
      if (generation === this.generation) this.patch(target, { status: 'failed', error: ipcErrorMessage(error) || 'unavailable' });
    }
  }

  private patch(target: Pick<TriageTarget, 'teamId' | 'sharedBotId'>, change: Partial<TriageLogSection> | ((section: TriageLogSection) => Partial<TriageLogSection>)): void {
    const key = sectionKey(target);
    this.update({
      sections: this.getSnapshot().sections.map((section) => sectionKey(section.target) === key ? { ...section, ...(typeof change === 'function' ? change(section) : change) } : section),
    });
  }
}
