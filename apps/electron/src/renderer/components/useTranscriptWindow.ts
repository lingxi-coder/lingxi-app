import { useCallback, useLayoutEffect, useMemo, useRef, useState, type MutableRefObject, type RefObject } from 'react';

/** Short transcripts keep their original DOM and spacing. History is never discarded. */
export const TRANSCRIPT_WINDOW_THRESHOLD = 160;
const OVERSCAN = 12;
const INITIAL_TAIL_ROWS = 40;
const ESTIMATED_HEIGHT = 96;

interface WindowRow {
  readonly id: string;
  readonly type: string;
  readonly role?: string;
}
interface Range { start: number; end: number }
interface WindowState extends Range { sessionKey: string; firstId?: string; lastId?: string }

/** Mirrors the transcript's sibling spacing, including the first mounted row. */
export function transcriptRowGap(previous: WindowRow | undefined, row: WindowRow): number {
  if (!previous) return 0;
  if (previous.role === 'user') return 1.5;
  if (row.role === 'user') return 1;
  const compact = (type: string) => type === 'tool-group' || type === 'agents' || type === 'skill-instructions';
  if (compact(previous.type) && compact(row.type)) return 0.125;
  if (compact(previous.type) || compact(row.type) || previous.type === 'thinking' || row.type === 'thinking') return 0.75;
  return 1;
}

function indexAt(offsets: readonly number[], offset: number): number {
  let low = 0;
  let high = Math.max(0, offsets.length - 2);
  while (low < high) {
    const middle = (low + high + 1) >>> 1;
    if (offsets[middle]! <= offset) low = middle;
    else high = middle - 1;
  }
  return low;
}

export function transcriptWindowRange(offsets: readonly number[], offset: number, height: number): Range {
  const count = offsets.length - 1;
  if (count <= 0) return { start: 0, end: 0 };
  return {
    start: Math.max(0, indexAt(offsets, Math.max(0, offset)) - OVERSCAN),
    end: Math.min(count, indexAt(offsets, Math.max(0, offset) + height) + OVERSCAN + 1),
  };
}

/**
 * A measured presentation window. Only mounted rows are measured/observed. The
 * cache owns heights, not messages or native UI revisions; unmounting a row uses
 * its existing native-surface cleanup, and remounting obtains a fresh revision.
 */
export function useTranscriptWindow<T extends WindowRow>({ rows, sessionKey, viewport, feed, following, onLayout }: {
  rows: readonly T[];
  sessionKey: string;
  viewport: RefObject<HTMLDivElement>;
  feed: RefObject<HTMLDivElement>;
  following: MutableRefObject<boolean>;
  onLayout(): void;
}) {
  const enabled = rows.length > TRANSCRIPT_WINDOW_THRESHOLD;
  const beforeRef = useRef<HTMLDivElement>(null);
  const cache = useRef({ sessionKey, revision: 0, width: 0, viewportHeight: 0, rem: 16, heights: new Map<string, number>() });
  if (cache.current.sessionKey !== sessionKey) cache.current = { sessionKey, revision: 0, width: 0, viewportHeight: 0, rem: 16, heights: new Map() };
  const [revision, setRevision] = useState(0);
  const [window, setWindow] = useState<WindowState>({ sessionKey, start: 0, end: 0 });
  const detachedPosition = useRef<{ sessionKey: string; id: string } | null>(null);
  const layout = useMemo(() => {
    const offsets = [0];
    const indices = new Map<string, number>();
    for (let index = 0; index < rows.length; index++) {
      const row = rows[index]!;
      indices.set(row.id, index);
      const height = cache.current.heights.get(row.id) ?? (row.type === 'tool-group' ? 40 : ESTIMATED_HEIGHT);
      offsets.push(offsets[index]! + height + transcriptRowGap(rows[index - 1], row) * cache.current.rem);
    }
    return { offsets, indices, revision: cache.current.revision };
  }, [rows, sessionKey, revision]);

  let range: Range = { start: 0, end: rows.length };
  if (enabled) {
    if (following.current) {
      const height = cache.current.viewportHeight || 720;
      const tail = transcriptWindowRange(layout.offsets, Math.max(0, layout.offsets[rows.length]! - height), height);
      range = { start: Math.min(tail.start, Math.max(0, rows.length - INITIAL_TAIL_ROWS)), end: rows.length };
    } else if (window.sessionKey === sessionKey && window.end > 0
      && !layout.indices.has(window.firstId ?? '') && layout.indices.has(window.lastId ?? '')) {
      // A prepended tool can merge with the first activity and change its row
      // id. Keep the surviving end of the current window mounted so Stage can
      // restore its reading anchor before refreshing the measured range.
      const end = layout.indices.get(window.lastId!)! + 1;
      range = { start: Math.max(0, end - (window.end - window.start) - OVERSCAN), end };
    } else if (window.sessionKey !== sessionKey || window.end === 0 || !layout.indices.has(window.firstId ?? '')) {
      // Crossing the threshold while backscrolling must keep the old reading
      // row mounted on the very first virtualized commit, before any effects.
      const held = detachedPosition.current;
      const index = held?.sessionKey === sessionKey ? layout.indices.get(held.id) : undefined;
      const start = index === undefined ? Math.max(0, rows.length - INITIAL_TAIL_ROWS) : Math.max(0, index - OVERSCAN);
      range = { start, end: Math.min(rows.length, start + Math.max(INITIAL_TAIL_ROWS, Math.ceil((viewport.current?.clientHeight || 720) / 24) + OVERSCAN * 2)) };
    } else {
      const start = layout.indices.get(window.firstId ?? '') ?? Math.min(window.start, rows.length - 1);
      const end = (layout.indices.get(window.lastId ?? '') ?? Math.max(start, window.end - 1)) + 1;
      range = { start, end: Math.min(rows.length, end) };
    }
  }
  const current = useRef({ rows, sessionKey, enabled, layout, range });
  current.current = { rows, sessionKey, enabled, layout, range };

  const select = useCallback((next: Range) => {
    const state = current.current;
    if (state.range.start === next.start && state.range.end === next.end) return;
    const active = document.activeElement;
    if (active instanceof HTMLElement && viewport.current?.contains(active)) {
      const index = Number(active.closest<HTMLElement>('[data-transcript-index]')?.dataset.transcriptIndex);
      if (Number.isFinite(index) && (index < next.start || index >= next.end)) viewport.current.focus({ preventScroll: true });
    }
    setWindow({ ...next, sessionKey: state.sessionKey, firstId: state.rows[next.start]?.id, lastId: state.rows[next.end - 1]?.id });
  }, [viewport]);

  const refresh = useCallback(() => {
    const state = current.current;
    const node = viewport.current;
    const before = beforeRef.current;
    if (!node || following.current) return;
    // A measurement queued a new layout. Its previous estimates cannot choose
    // the next visible range or evict the current reading anchor.
    if (state.layout.revision !== cache.current.revision) return;
    if (!state.enabled) {
      const top = node.getBoundingClientRect().top;
      const mounted = feed.current?.querySelectorAll<HTMLElement>(':scope > .transcript-row:not([data-thinking-slot])');
      const index = Array.from(mounted ?? []).findIndex(row => row.getBoundingClientRect().bottom > top);
      const row = state.rows[index];
      if (row) detachedPosition.current = { sessionKey: state.sessionKey, id: row.id };
      return;
    }
    if (!before) return;
    // The spacer's top is the logical row origin, after feed padding/headings.
    const origin = before.getBoundingClientRect().top - node.getBoundingClientRect().top + node.scrollTop;
    select(transcriptWindowRange(state.layout.offsets, node.scrollTop - origin, node.clientHeight));
  }, [select, viewport, feed, following]);

  const reveal = useCallback((id: string): boolean => {
    const state = current.current;
    const index = state.layout.indices.get(id);
    if (!state.enabled || index === undefined || (index >= state.range.start && index < state.range.end)) return false;
    select({ start: Math.max(0, index - OVERSCAN), end: Math.min(state.rows.length, index + OVERSCAN + 1) });
    return true;
  }, [select]);

  const measure = useCallback(() => {
    if (!current.current.enabled || !feed.current) return;
    let changed = false;
    const width = feed.current.clientWidth;
    const height = viewport.current?.clientHeight ?? 0;
    const rem = Number.parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
    if (Math.abs(cache.current.width - width) > 0.5 || cache.current.rem !== rem) {
      cache.current.heights.clear();
      cache.current.width = width;
      cache.current.rem = rem;
      changed = true;
    }
    if (cache.current.viewportHeight !== height) {
      cache.current.viewportHeight = height;
      changed = true;
    }
    for (const node of feed.current.querySelectorAll<HTMLElement>(':scope > [data-transcript-id]')) {
      const height = node.getBoundingClientRect().height;
      const id = node.dataset.transcriptId!;
      if (Math.abs((cache.current.heights.get(id) ?? -1) - height) > 0.5) {
        cache.current.heights.set(id, height);
        changed = true;
      }
    }
    if (changed) {
      cache.current.revision++;
      setRevision(value => value + 1);
    }
  }, [feed, viewport]);

  // Measure before Stage restores its text anchor or synchronizes the tail.
  useLayoutEffect(measure);
  useLayoutEffect(() => {
    if (!enabled || !feed.current || !viewport.current) return;
    const observer = new ResizeObserver(() => {
      measure();
      onLayout();
    });
    observer.observe(viewport.current);
    for (const node of feed.current.querySelectorAll(':scope > [data-transcript-id]')) observer.observe(node);
    return () => observer.disconnect();
  }, [enabled, sessionKey, range.start, range.end, measure, feed, viewport, onLayout]);

  return {
    enabled, start: range.start, end: range.end, beforeRef, refresh, reveal,
    before: layout.offsets[range.start] ?? 0,
    after: (layout.offsets[rows.length] ?? 0) - (layout.offsets[range.end] ?? 0),
  };
}
