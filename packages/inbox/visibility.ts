import { browserInboxStorage, type InboxStorage } from './storage.js';
/** Source-order cursor. A higher visible row cannot acknowledge an unseen intervening row. */
export class VisibleReadFrontier {
  private visible = new Map<number, number>();
  private confirmed: Array<[number, number]> = [];
  private sent: number;
  private lastWrite = -Infinity;
  private timer?: ReturnType<typeof setTimeout>;
  private closed = false;
  constructor(
    private through: number,
    readonly manualUnreadRevision: number,
    private commit: (through: number, manualUnreadRevision: number) => void,
    private now = Date.now,
    private remember?: (ranges: Array<[number, number]>) => void,
  ) {
    this.sent = through;
  }
  observe(seq: number, inView: boolean) {
    if (this.closed || seq <= this.through) return;
    this.tick();
    if (inView) {
      if (!this.visible.has(seq) && !this.confirmed.some(([lo, hi]) => seq >= lo && seq <= hi))
        this.visible.set(seq, this.now());
    } else this.visible.delete(seq);
    this.schedule();
  }
  advance(through: number) {
    this.through = Math.max(this.through, through);
    this.sent = Math.max(this.sent, through);
    this.confirmed = this.confirmed.filter(([, hi]) => hi > this.through);
    for (const n of this.visible.keys()) if (n <= this.through) this.visible.delete(n);
    this.tick();
  }
  restore(ranges: Array<[number, number]>) {
    if (this.closed) return;
    this.confirmed = mergeRanges([...this.confirmed, ...ranges], this.through);
    this.tick();
    this.schedule();
  }
  private tick() {
    const now = this.now();
    const qualified: Array<[number, number]> = [];
    for (const [seq, since] of this.visible)
      if (now - since >= 300) {
        qualified.push([seq, seq]);
        this.visible.delete(seq);
      }
    if (qualified.length) {
      this.confirmed = mergeRanges([...this.confirmed, ...qualified], this.through);
      this.remember?.(qualified);
    }
    while (this.confirmed[0] && this.confirmed[0]![0] <= this.through + 1) {
      this.through = Math.max(this.through, this.confirmed.shift()![1]);
    }
    if (this.sent < this.through && now - this.lastWrite >= 1000) this.flush();
  }
  private schedule() {
    clearTimeout(this.timer);
    let due = this.sent < this.through ? this.lastWrite + 1000 : Infinity;
    for (const since of this.visible.values()) due = Math.min(due, since + 300);
    if (Number.isFinite(due))
      this.timer = setTimeout(
        () => {
          this.tick();
          this.schedule();
        },
        Math.max(0, due - this.now()),
      );
  }
  private flush() {
    if (this.sent >= this.through) return;
    this.sent = this.through;
    this.lastWrite = this.now();
    this.commit(this.through, this.manualUnreadRevision);
  }
  leave() {
    this.tick();
    this.visible.clear();
    this.flush();
    clearTimeout(this.timer);
  }
  close() {
    this.leave();
    this.closed = true;
  }
}

export function mergeRanges(ranges: Array<[number, number]>, through: number): Array<[number, number]> {
  const merged: Array<[number, number]> = [];
  for (const [lo, hi] of ranges.sort((a, b) => a[0] - b[0])) {
    if (!Number.isSafeInteger(lo) || !Number.isSafeInteger(hi) || hi < lo || hi <= through) continue;
    const prev = merged[merged.length - 1];
    if (prev && lo <= prev[1] + 1) prev[1] = Math.max(prev[1], hi);
    else merged.push([Math.max(through + 1, lo), hi]);
  }
  return merged;
}

/** Compressed source intervals live outside the evictable page cache. Transactions union exposures from multiple tabs. */
export function durableReadFrontier(
  account: string,
  key: string,
  generation: number,
  through: number,
  manualRevision: number,
  commit: (through: number, revision: number) => void,
  onError: (error: unknown) => void,
  storage: InboxStorage = browserInboxStorage,
) {
  const partition = JSON.stringify(['inbox-exposure', account, key, generation]);
  const save = (ranges: Array<[number, number]>) =>
    storage.update(partition, (record) => {
      if (record.exposure && record.exposure.revision > manualRevision) return [];
      const previous = record.exposure?.revision === manualRevision ? record.exposure.ranges : [];
      const merged = mergeRanges([...previous, ...ranges], through);
      record.exposure = { revision: manualRevision, ranges: merged };
      return merged;
    });
  const frontier = new VisibleReadFrontier(through, manualRevision, commit, Date.now, (ranges) => {
    void save(ranges).catch(onError);
  });
  void save([])
    .then((ranges) => frontier.restore(ranges))
    .catch(onError);
  return frontier;
}

/** The observer follows rendered message elements only. Background tabs and blurred windows never accumulate dwell time. */
export function observeTranscript(
  root: HTMLElement,
  frontier: VisibleReadFrontier,
  options?: {
    selector?: string;
    cursors?: (node: HTMLElement) => readonly number[];
    onRecheck?: (recheck: () => void) => () => void;
    isForeground?: () => boolean;
  },
) {
  if (typeof IntersectionObserver === 'undefined') return () => frontier.close();
  const intersecting = new Set<HTMLElement>(),
    tracked = new Set<HTMLElement>();
  let previous = new Set<number>();
  const selector = options?.selector ?? '[data-message-seq]';
  const foreground = () => document.visibilityState === 'visible' && document.hasFocus() && (options?.isForeground?.() ?? true);
  function recheck() {
    const next = new Set<number>();
    if (foreground())
      for (const node of intersecting)
        for (const seq of options?.cursors?.(node) ?? [Number(node.dataset.messageSeq)]) {
          if (Number.isSafeInteger(seq) && seq > 0) next.add(seq);
        }
    for (const seq of previous) if (!next.has(seq)) frontier.observe(seq, false);
    for (const seq of next) if (!previous.has(seq)) frontier.observe(seq, true);
    previous = next;
    if (!foreground()) frontier.leave();
  }
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const node = entry.target as HTMLElement;
        if (!tracked.has(node)) continue;
        if (
          entry.isIntersecting &&
          entry.intersectionRect.height > 0 &&
          entry.intersectionRect.height >= Math.min(16, entry.boundingClientRect.height)
        )
          intersecting.add(node);
        else intersecting.delete(node);
      }
      recheck();
    },
    { root, threshold: [0, 0.01, 0.25, 0.5, 1] },
  );
  function scan() {
    for (const node of tracked)
      if (!root.contains(node) || !node.matches(selector)) {
        observer.unobserve(node);
        tracked.delete(node);
        intersecting.delete(node);
      }
    for (const node of root.querySelectorAll<HTMLElement>(selector))
      if (!tracked.has(node)) {
        tracked.add(node);
        observer.observe(node);
      }
    recheck();
  }
  scan();
  const mutation = new MutationObserver(scan);
  mutation.observe(root, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ['data-inbox-uuids', 'data-message-seq'],
  });
  const stopRecheck = options?.onRecheck?.(recheck);
  window.addEventListener('focus', recheck);
  window.addEventListener('blur', recheck);
  document.addEventListener('visibilitychange', recheck);
  return () => {
    observer.disconnect();
    mutation.disconnect();
    stopRecheck?.();
    window.removeEventListener('focus', recheck);
    window.removeEventListener('blur', recheck);
    document.removeEventListener('visibilitychange', recheck);
    frontier.close();
  };
}
