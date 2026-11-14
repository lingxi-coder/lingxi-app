import { createContext, useCallback, useContext, useLayoutEffect, useMemo, useRef, useState, type MutableRefObject, type ReactNode, type RefObject } from 'react';
import { transcriptRowUuids } from '../model/runItem';
import './TranscriptLeafWindow.css';

export const TRANSCRIPT_LEAF_WINDOW_THRESHOLD = 64;
const OVERSCAN = 8;
interface Leaf {
  readonly id: string;
  readonly type: string;
  readonly transcriptUuid?: string;
  readonly transcriptUuids?: readonly string[];
}
interface Range { start: number; end: number }
interface Scope {
  sessionKey: string;
  viewport: RefObject<HTMLDivElement>;
  following: MutableRefObject<boolean>;
  targetId?: string;
  targetPending(): boolean;
  readingId(): string | undefined;
  /** Shares Stage's character anchor and follow-tail writer after child commits. */
  onLayout(): void;
}
export const TranscriptLeafWindowScope = createContext<Scope | null>(null);

function indexAt(offsets: readonly number[], offset: number): number {
  let low = 0, high = Math.max(0, offsets.length - 2);
  while (low < high) {
    const middle = (low + high + 1) >>> 1;
    if (offsets[middle]! <= offset) low = middle;
    else high = middle - 1;
  }
  return low;
}

export function transcriptLeafRange(offsets: readonly number[], top: number, height: number): Range {
  const count = Math.max(0, offsets.length - 1);
  return count ? {
    start: Math.max(0, indexAt(offsets, Math.max(0, top)) - OVERSCAN),
    end: Math.min(count, indexAt(offsets, Math.max(0, top + height)) + OVERSCAN + 1),
  } : { start: 0, end: 0 };
}

/**
 * A measured window in the transcript's scrollport, never a second scroller.
 * Only heights and stable IDs are cached. ToolCall owns native-site retirement
 * on unmount and obtains fresh revisions when a leaf returns to the window.
 */
export function TranscriptLeafWindow<T extends Leaf>({ items, renderItem, gap = 0 }: {
  items: readonly T[];
  renderItem(item: T): ReactNode;
  gap?: number;
}) {
  const scope = useContext(TranscriptLeafWindowScope);
  const enabled = Boolean(scope) && items.length > TRANSCRIPT_LEAF_WINDOW_THRESHOLD;
  const root = useRef<HTMLDivElement>(null);
  const sessionKey = scope?.sessionKey ?? '';
  const cache = useRef({ sessionKey, revision: 0, width: 0, heights: new Map<string, number>() });
  if (cache.current.sessionKey !== sessionKey) cache.current = { sessionKey, revision: 0, width: 0, heights: new Map() };
  const [revision, setRevision] = useState(0);
  const [selection, setSelection] = useState<{ sessionKey: string; first?: string; last?: string }>({ sessionKey });
  const layout = useMemo(() => {
    const offsets = [0], indices = new Map<string, number>();
    items.forEach((item, index) => {
      indices.set(item.id, index);
      offsets.push(offsets[index]! + (cache.current.heights.get(item.id) ?? (item.type === 'tool' ? 36 : 120)) + (index < items.length - 1 ? gap : 0));
    });
    return { offsets, indices, revision: cache.current.revision };
  }, [items, sessionKey, revision, gap]);

  let range: Range = { start: 0, end: items.length };
  if (enabled && scope) {
    const target = scope.targetPending() ? layout.indices.get(scope.targetId ?? '') : undefined;
    const reading = layout.indices.get(scope.readingId() ?? '');
    const first = selection.sessionKey === sessionKey ? layout.indices.get(selection.first ?? '') : undefined;
    const last = selection.sessionKey === sessionKey ? layout.indices.get(selection.last ?? '') : undefined;
    if (target !== undefined || reading !== undefined && first === undefined) {
      const index = target ?? reading!;
      range = { start: Math.max(0, index - OVERSCAN), end: Math.min(items.length, index + OVERSCAN + 1) };
    } else if (scope.following.current) {
      const height = scope.viewport.current?.clientHeight || 720;
      range = transcriptLeafRange(layout.offsets, layout.offsets[items.length]! - height, height);
    } else if (first !== undefined && last !== undefined) range = { start: first, end: last + 1 };
    else range = { start: 0, end: Math.min(items.length, 32) };
  }
  const current = useRef({ scope, enabled, layout, items, range });
  current.current = { scope, enabled, layout, items, range };

  const refresh = useCallback(() => {
    const state = current.current;
    const viewport = state.scope?.viewport.current;
    if (!state.enabled || !state.scope || !viewport || !root.current || state.scope.following.current || state.scope.targetPending()) return;
    if (state.layout.revision !== cache.current.revision) return;
    const bounds = root.current.getBoundingClientRect();
    const top = viewport.getBoundingClientRect().top - bounds.top;
    const next = transcriptLeafRange(state.layout.offsets, top, viewport.clientHeight);
    if (state.range.start === next.start && state.range.end === next.end) return;
    const active = document.activeElement;
    if (active instanceof HTMLElement && root.current.contains(active)) {
      const index = state.layout.indices.get(active.closest<HTMLElement>('[data-transcript-leaf-id]')?.dataset.transcriptLeafId ?? '');
      if (index !== undefined && (index < next.start || index >= next.end)) viewport.focus({ preventScroll: true });
    }
    const selected = { sessionKey: state.scope.sessionKey, first: state.items[next.start]?.id, last: state.items[next.end - 1]?.id };
    setSelection(previous => previous.sessionKey === selected.sessionKey && previous.first === selected.first && previous.last === selected.last ? previous : selected);
  }, []);

  const measure = useCallback(() => {
    if (!current.current.enabled || !root.current) return;
    let changed = false;
    const width = root.current.clientWidth;
    if (Math.abs(cache.current.width - width) > 0.5) {
      cache.current.width = width;
      cache.current.heights.clear();
      changed = true;
    }
    for (const node of root.current.querySelectorAll<HTMLElement>(':scope > [data-transcript-leaf-id]')) {
      const id = node.dataset.transcriptLeafId!;
      const height = node.getBoundingClientRect().height - Number(node.dataset.leafGap || 0);
      if (Math.abs((cache.current.heights.get(id) ?? -1) - height) > 0.5) {
        cache.current.heights.set(id, height);
        changed = true;
      }
    }
    if (changed) {
      cache.current.revision++;
      setRevision(value => value + 1);
    }
  }, []);

  useLayoutEffect(() => {
    measure();
    if (enabled) scope?.onLayout();
    // A reveal owns the selected window until Stage has located the leaf.
    // Retain that range after location clears the request; the last scroll
    // selection may still refer to the opposite end of the same activity.
    if (enabled && scope?.targetPending() && layout.indices.has(scope.targetId ?? '')) {
      const selected = { sessionKey, first: items[range.start]?.id, last: items[range.end - 1]?.id };
      setSelection(previous => previous.sessionKey === selected.sessionKey && previous.first === selected.first && previous.last === selected.last ? previous : selected);
    } else refresh();
  });
  useLayoutEffect(() => {
    const viewport = scope?.viewport.current;
    if (!enabled || !viewport || !root.current) return;
    const onScroll = () => refresh();
    viewport.addEventListener('scroll', onScroll, { passive: true });
    const observer = new ResizeObserver(() => {
      measure();
      current.current.scope?.onLayout();
      refresh();
    });
    observer.observe(viewport);
    observer.observe(root.current);
    for (const node of root.current.querySelectorAll(':scope > [data-transcript-leaf-id]')) observer.observe(node);
    return () => { viewport.removeEventListener('scroll', onScroll); observer.disconnect(); };
  }, [enabled, sessionKey, range.start, range.end, scope?.viewport, measure, refresh]);

  return <div ref={root} className="transcript-leaf-window" data-leaf-windowed={enabled || undefined}
    role={enabled ? 'list' : undefined}>
    {enabled && <div className="transcript-leaf-spacer" aria-hidden="true" style={{ height: layout.offsets[range.start] }} />}
    {items.slice(range.start, range.end).map((item, mountedIndex) => {
      const index = range.start + mountedIndex;
      const uuids = transcriptRowUuids(item);
      const leafGap = index < items.length - 1 ? gap : 0;
      return <div key={item.id} className="transcript-leaf" data-transcript-leaf-id={item.id}
        data-message-uuid={uuids[0]} data-message-uuids={uuids.length ? uuids.join(' ') : undefined}
        data-leaf-gap={leafGap || undefined} style={leafGap ? { paddingBottom: leafGap } : undefined}
        role={enabled ? 'listitem' : undefined} aria-posinset={enabled ? index + 1 : undefined} aria-setsize={enabled ? items.length : undefined}>
        {renderItem(item)}
      </div>;
    })}
    {enabled && <div className="transcript-leaf-spacer" aria-hidden="true" style={{ height: layout.offsets[items.length]! - layout.offsets[range.end]! }} />}
  </div>;
}
