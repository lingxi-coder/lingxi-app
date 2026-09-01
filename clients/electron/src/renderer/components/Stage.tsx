import { memo, useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { useT } from '../theme/ThemeContext';
import type { CommandRunItem, RunItem } from '../model/runItem';
import { commandShouldCollapse, narrationDefaultOpen, narrationShouldCollapse } from '../model/runItem';
import { collapseFor, collapseInitial, collapseOpen, collapseSet } from './collapseStore';
import { Icon } from './Icon';
import { Disclosure } from './Disclosure';
import { MarkdownContent } from './MarkdownContent';
import { ToolCall } from './ToolCall';

// ─── RUN ITEMS ───────────────────────────────────────────────
const NarrationLine = memo(function NarrationLine({ item, open, onSetOpen }: {
  item: Extract<RunItem, { type: 'narration' }>;
  open: boolean;
  onSetOpen(id: string, next: boolean): void;
}) {
  const t = useT();
  const user = item.role === 'user';
  const color = item.tone === 'muted' ? t.text3 : t.text;
  const images = item.images?.filter((image) => image.url.trim().length > 0) ?? [];
  const collapsible = narrationShouldCollapse(item);
  const expanded = !collapsible || open;
  const contentId = `narration-content-${item.id}`;
  return (
    <div className={user ? 'user-message-bubble' : undefined} style={{
      maxWidth: user ? images.length ? 430 : 700 : 880,
      padding: user ? '11px 16px' : 0,
      borderRadius: user ? 22 : 0,
      border: 0,
      background: user ? t.surfaceHover : 'transparent',
      fontSize: 15, lineHeight: 1.68, color, fontWeight: item.strong ? 500 : 400,
    }}>
      {images.length > 0 && (
        <div role="group" aria-label="Attached images" style={{ display: 'grid', gridTemplateColumns: images.length > 1 ? 'repeat(2, minmax(0, 1fr))' : 'minmax(0, 1fr)', gap: 7, marginBottom: item.text ? 8 : 0 }}>
          {images.map((image, index) => (
            <div key={`${image.media_type}-${index}`} style={{ overflow: 'hidden', minWidth: 0, borderRadius: 11, background: t.surfaceActive, outline: `1px solid color-mix(in oklab, ${t.text} 12%, transparent)` }}>
              <img
                src={image.url}
                alt={`Attached image ${index + 1}`}
                style={{ display: 'block', width: '100%', maxHeight: 260, aspectRatio: images.length > 1 ? '4 / 3' : 'auto', objectFit: 'contain', background: t.surfaceActive, outline: `1px solid color-mix(in oklab, ${t.text} 8%, transparent)`, outlineOffset: -1 }}
              />
            </div>
          ))}
        </div>
      )}
      <div
        id={contentId}
        style={{
          maxHeight: expanded ? undefined : '13.6em',
          overflow: expanded ? undefined : 'hidden',
          WebkitMaskImage: expanded
            ? undefined
            : 'linear-gradient(to bottom, #000 0%, #000 78%, transparent 100%)',
          maskImage: expanded
            ? undefined
            : 'linear-gradient(to bottom, #000 0%, #000 78%, transparent 100%)',
          textWrap: 'pretty',
        }}
      >
        <MarkdownContent text={item.text} />
      </div>
      {collapsible && (
        <button
          type="button"
          className="narration-disclosure-trigger"
          aria-expanded={expanded}
          aria-controls={contentId}
          aria-label={expanded ? 'Collapse full message' : 'Show full message'}
          onClick={() => onSetOpen(item.id, !expanded)}
          style={{ color: user ? t.accent : t.text3 }}
        >
          <span>{expanded ? 'Show less' : 'Show more'}</span>
          <Icon name={expanded ? 'chevron' : 'chevronR'} size={12} stroke={2} />
        </button>
      )}
    </div>
  );
});

// ─── THINKING BLOCK (collapsible, dim/italic reasoning stream) ──────
//
// The open/closed state lives in the Stage's store, keyed by the block's stable
// id — NOT in this component. It used to auto-collapse the instant the block
// sealed, which fought a user who had deliberately opened it to read along; the
// DEFAULT now comes from `streamed` (a field that never flips) and any explicit
// user choice wins over it forever.
const ThinkingBlock = memo(function ThinkingBlock({ item, open, onSetOpen }: {
  item: Extract<RunItem, { type: 'thinking' }>;
  open: boolean;
  onSetOpen(id: string, next: boolean): void;
}) {
  const t = useT();
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6, maxWidth: 880 }}>
      <Disclosure
        id={item.id}
        open={open}
        onToggle={() => onSetOpen(item.id, !open)}
        buttonStyle={{ padding: '2px 6px 2px 2px', borderRadius: 6, fontSize: 12.5, fontWeight: 500 }}
        summary={
          <span
            className={item.done ? undefined : 'running-sweep'}
            style={item.done ? undefined : {
              '--sweep-base': t.text3,
              '--sweep-highlight': t.text,
            } as CSSProperties}
          >
            {item.done ? 'Thought' : 'Thinking…'}
          </span>
        }
      >
        <div
          style={{
            borderLeft: `2px solid ${t.border}`, paddingLeft: 12, marginLeft: 6,
            fontSize: 14, lineHeight: 1.6, color: t.text3, fontStyle: 'italic',
            whiteSpace: 'pre-wrap',
          }}
        >
          {item.text}
          {!item.done && (
            <span style={{ animation: 'cursor-blink 1.1s step-end infinite' }}>▍</span>
          )}
        </div>
      </Disclosure>
    </div>
  );
});

const CommandOutput = memo(function CommandOutput({ item, open, onSetOpen }: {
  item: CommandRunItem;
  open: boolean;
  onSetOpen: (id: string, next: boolean) => void;
}) {
  const t = useT();
  // Deliberately NOT MarkdownContent: /help and /status are column-aligned
  // plain text and a markdown pass destroys the alignment.
  const body = (
    <pre
      className="mono"
      style={{
        margin: 0, whiteSpace: 'pre-wrap', wordBreak: 'break-word',
        fontSize: 12, lineHeight: 1.55,
        color: item.isError ? t.danger : t.text2,
      }}
    >{item.output}</pre>
  );
  if (!commandShouldCollapse(item)) return body;
  return (
    <Disclosure
      id={item.id}
      open={open}
      onToggle={() => onSetOpen(item.id, !open)}
      summary={item.name || 'Command output'}
    >
      {body}
    </Disclosure>
  );
});

/** Whether a thinking block starts open, before any user choice. */
function thinkingDefaultOpen(item: Extract<RunItem, { type: 'thinking' }>): boolean {
  return item.streamed === true;
}

// ─── STAGE (the agent run scrollback) ────────────────────────
interface StageProps {
  /** The real conversation accumulated from the bridge. */
  liveItems?: RunItem[];
  /** True while a turn is streaming — shows the thinking affordance at the tail. */
  running?: boolean;
  /** Truthful empty/onboarding copy supplied by the host state. */
  emptyMessage?: string;
  /**
   * Which session `liveItems` belongs to (`ConversationState.sessionKey`).
   * Item ids restart at `i1` on every session change, so the collapse map is
   * scoped by this and dropped when it changes.
   */
  sessionKey?: string;
}

export function Stage({ liveItems = [], running = false, emptyMessage = 'Start a new conversation when the engine is ready.', sessionKey = '' }: StageProps) {
  const t = useT();
  const tailRef = useRef<HTMLDivElement>(null);
  const items: RunItem[] = liveItems;

  /**
   * Explicit open/closed choices, keyed by the item's STABLE id WITHIN a
   * session. This has to live above the rows: the list recycles them, so a
   * `useState` inside a row would hand its state to whatever item later
   * occupies that position. Absent key ⇒ fall back to the item's own default.
   *
   * The session scope is not decoration — the reducer's ids restart at `i1`
   * for every new session, so an unscoped map applied the previous session's
   * choices to whatever landed at the same id in the next one.
   */
  const [collapse, setCollapse] = useState(() => collapseInitial(sessionKey));
  // Derived during render — no effect, so the very first paint of a new
  // session is already clean rather than clean one frame later.
  const visible = collapseFor(collapse, sessionKey);
  // The setter must not change identity (a per-row arrow function would defeat
  // the rows' React.memo), so the live session key reaches it through a ref.
  const sessionRef = useRef(sessionKey);
  sessionRef.current = sessionKey;
  // ONE stable callback for every row. Rows pass the value they want rather
  // than a bare "toggle", because only the row knows the default it started
  // from.
  const setOpen = useCallback((id: string, next: boolean) => {
    setCollapse((previous) => collapseSet(previous, sessionRef.current, id, next));
  }, []);

  // Keep the newest content in view as deltas stream in.
  useEffect(() => {
    tailRef.current?.scrollIntoView({ block: 'end' });
  }, [items.length, running]);

  return (
    <div style={{ flex: 1, overflowY: 'auto', background: t.transcriptBg, position: 'relative' }}>
      <div
        style={{
          maxWidth: 920, margin: '0 auto',
          padding: '28px 32px 12px',
          display: 'flex', flexDirection: 'column', gap: 18,
        }}
      >
        {items.length === 0 && !running && (
          <div
            role="status"
            style={{
              minHeight: 260, display: 'flex', alignItems: 'center', justifyContent: 'center',
              color: t.text3, fontSize: 14, textAlign: 'center', lineHeight: 1.6,
            }}
          >
            {emptyMessage}
          </div>
        )}
        {/*
          Keyed on the item's stable id, never the array index. An index key
          silently reassigns every row's collapse state the moment an item is
          inserted, which is exactly what a streaming transcript does.
        */}
        {items.map((item) => {
          if (item.type === 'narration') {
            return (
              <div key={item.id} style={{ display: 'flex', justifyContent: item.role === 'user' ? 'flex-end' : 'flex-start', gap: 10, width: '100%', animation: 'fade-in 0.3s ease' }}>
                <NarrationLine
                  item={item}
                  open={collapseOpen(visible, sessionKey, item.id) ?? narrationDefaultOpen(item)}
                  onSetOpen={setOpen}
                />
              </div>
            );
          }
          if (item.type === 'thinking') {
            return (
              <div key={item.id} style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <ThinkingBlock
                    item={item}
                    open={collapseOpen(visible, sessionKey, item.id) ?? thinkingDefaultOpen(item)}
                    onSetOpen={setOpen}
                  />
                </div>
              </div>
            );
          }
          if (item.type === 'tool') {
            return (
              <div key={item.id} style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <ToolCall item={item} open={collapseOpen(visible, sessionKey, item.id)} onSetOpen={setOpen} />
                </div>
              </div>
            );
          }
          if (item.type === 'command') {
            return (
              <div key={item.id} style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <CommandOutput
                    item={item}
                    open={collapseOpen(visible, sessionKey, item.id) ?? false}
                    onSetOpen={setOpen}
                  />
                </div>
              </div>
            );
          }
          return null;
        })}

        {/* Streaming affordance — shown at the tail while a live turn runs. */}
        {running && (
          <div style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
            <div style={{ display: 'flex', alignItems: 'center', color: t.text3, fontSize: 13.5 }}>
              <span
                className="running-sweep"
                style={{
                  '--sweep-base': t.text3,
                  '--sweep-highlight': t.text,
                } as CSSProperties}
              >
                Thinking…
              </span>
            </div>
          </div>
        )}

        {/* Scroll anchor — keeps the newest content in view as deltas arrive. */}
        <div ref={tailRef} />
      </div>
    </div>
  );
}
