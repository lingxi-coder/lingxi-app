import { useState, useEffect, useRef } from 'react';
import { useT } from '../theme/ThemeContext';
import type { RunItem } from '../data';
import { Icon } from './Icon';
import { MarkdownContent } from './MarkdownContent';

// ─── RUN ITEMS ───────────────────────────────────────────────
function GutterRule() {
  const t = useT();
  // a small monospace tick column like the screenshot's "—" marks
  return (
    <div
      style={{
        width: 22, flexShrink: 0, paddingTop: 6,
        display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 14,
        color: t.text4,
      }}
    >
      <span className="mono" style={{ fontSize: 11, lineHeight: 1 }}>—</span>
    </div>
  );
}

function AgentCard({ item }: { item: Extract<RunItem, { type: 'agent' }> }) {
  const t = useT();
  const running = item.state === 'running';
  const [expanded, setExpanded] = useState(false);
  const expandable = Boolean(item.detail);
  return (
    <div
      style={{
        background: t.surface, border: `0.5px solid ${t.border}`,
        borderRadius: 10, padding: '10px 14px',
        display: 'flex', flexDirection: 'column', gap: 4,
        maxWidth: 640, position: 'relative', overflow: 'hidden',
      }}
    >
      {running && (
        <div
          style={{
            position: 'absolute', inset: 0, pointerEvents: 'none',
            background: `linear-gradient(180deg, transparent, ${t.accentBg}, transparent)`,
            animation: 'scan 2.4s linear infinite',
          }}
        />
      )}
      <button
        type="button"
        onClick={() => expandable && setExpanded((value) => !value)}
        aria-expanded={expandable ? expanded : undefined}
        style={{ display: 'flex', alignItems: 'center', gap: 8, position: 'relative', border: 0, padding: 0, background: 'transparent', color: 'inherit', font: 'inherit', textAlign: 'left', cursor: expandable ? 'pointer' : 'default' }}
      >
        {running ? (
          <span
            style={{
              width: 12, height: 12, borderRadius: 99, background: t.accent,
              boxShadow: `0 0 0 4px color-mix(in oklab, ${t.accent} 22%, transparent)`,
              animation: 'shimmer 1.3s infinite', flexShrink: 0,
            }}
          />
        ) : (
          <span
            style={{
              width: 16, height: 16, borderRadius: 4, flexShrink: 0,
              display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
              background: item.error ? t.danger : t.text3, color: t.windowBg,
            }}
          >
            <Icon name="check" size={11} stroke={3} />
          </span>
        )}
        <span style={{ fontSize: 14, fontWeight: 500, color: t.text }}>{item.title}</span>
        {expandable && <Icon name={expanded ? 'chevron' : 'chevronR'} size={13} color={t.text3} stroke={2} />}
      </button>
      {item.sub && (
        <div
          style={{
            fontSize: 14, color: item.link ? t.accent : t.text2,
            marginLeft: 24, fontWeight: 500,
            textDecoration: item.link ? 'underline' : 'none',
            textDecorationColor: item.link ? t.accentBorder : 'transparent',
            textUnderlineOffset: 3,
          }}
        >
          {item.sub}
        </div>
      )}
      {expanded && item.detail && (
        <pre
          className="mono"
          style={{
            margin: '6px 0 0 24px', padding: 10, maxHeight: 240, overflow: 'auto',
            whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', borderRadius: 7,
            background: t.windowBg, color: item.error ? t.danger : t.text2,
            border: `0.5px solid ${t.border}`, fontSize: 11.5, lineHeight: 1.5,
          }}
        >
          {item.detail}
        </pre>
      )}
    </div>
  );
}

function NarrationLine({ item }: { item: Extract<RunItem, { type: 'narration' }> }) {
  const t = useT();
  const user = item.role === 'user';
  const color = item.tone === 'muted' ? t.text3 : t.text;
  return (
    <div style={{
      maxWidth: user ? 700 : 880,
      padding: user ? '10px 14px' : 0,
      borderRadius: user ? '17px 17px 5px 17px' : 0,
      border: user ? `0.5px solid ${t.accentBorder}` : 0,
      background: user ? t.accentBg : 'transparent',
      fontSize: 14.5, lineHeight: 1.7, color, fontWeight: item.strong ? 500 : 400,
    }}>
      <MarkdownContent text={item.text} />
    </div>
  );
}

// ─── THINKING BLOCK (collapsible, dim/italic reasoning stream) ──────
function ThinkingBlock({ item }: { item: Extract<RunItem, { type: 'thinking' }> }) {
  const t = useT();
  // Auto-expanded while streaming so the reasoning is visible live; the user
  // can collapse it once sealed. We default-collapse a completed block.
  const [open, setOpen] = useState(!item.done);
  const wasDone = useRef(item.done);
  // Collapse automatically the moment the block seals (done flips true).
  useEffect(() => {
    if (item.done && !wasDone.current) setOpen(false);
    wasDone.current = item.done;
  }, [item.done]);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6, maxWidth: 880 }}>
      <button
        onClick={() => setOpen((o) => !o)}
        style={{
          display: 'inline-flex', alignItems: 'center', gap: 6, alignSelf: 'flex-start',
          padding: '2px 6px 2px 2px', borderRadius: 6, border: 'none', cursor: 'pointer',
          background: 'transparent', color: t.text3, fontSize: 12.5, fontWeight: 500,
        }}
        onMouseEnter={(e) => (e.currentTarget.style.color = t.text2)}
        onMouseLeave={(e) => (e.currentTarget.style.color = t.text3)}
      >
        <Icon name={open ? 'chevron' : 'chevronR'} size={13} stroke={2} />
        <Icon name="spark" size={12} stroke={1.8} />
        <span>{item.done ? 'Thought' : 'Thinking…'}</span>
      </button>
      {open && (
        <div
          style={{
            borderLeft: `2px solid ${t.border}`, paddingLeft: 12, marginLeft: 6,
            fontSize: 13.5, lineHeight: 1.65, color: t.text3, fontStyle: 'italic',
            whiteSpace: 'pre-wrap',
          }}
        >
          {item.text}
          {!item.done && (
            <span style={{ animation: 'cursor-blink 1.1s step-end infinite' }}>▍</span>
          )}
        </div>
      )}
    </div>
  );
}

// ─── STAGE (the agent run scrollback) ────────────────────────
interface StageProps {
  /** The real conversation accumulated from the bridge. */
  liveItems?: RunItem[];
  /** True while a turn is streaming — shows the thinking affordance at the tail. */
  running?: boolean;
  /** Truthful empty/onboarding copy supplied by the host state. */
  emptyMessage?: string;
}

export function Stage({ liveItems = [], running = false, emptyMessage = 'Start a new conversation when the engine is ready.' }: StageProps) {
  const t = useT();
  const tailRef = useRef<HTMLDivElement>(null);
  const items: RunItem[] = liveItems;

  // Keep the newest content in view as deltas stream in.
  useEffect(() => {
    tailRef.current?.scrollIntoView({ block: 'end' });
  }, [items.length, running]);

  return (
    <div style={{ flex: 1, overflowY: 'auto', background: t.stageBg, position: 'relative' }}>
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
        {items.map((item, i) => {
          if (item.type === 'narration') {
            return (
              <div key={i} style={{ display: 'flex', justifyContent: item.role === 'user' ? 'flex-end' : 'flex-start', gap: 10, width: '100%', animation: 'fade-in 0.3s ease' }}>
                {item.role !== 'user' && <GutterRule />}
                <NarrationLine item={item} />
              </div>
            );
          }
          if (item.type === 'thinking') {
            return (
              <div key={i} style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
                <GutterRule />
                <div style={{ flex: 1 }}>
                  <ThinkingBlock item={item} />
                </div>
              </div>
            );
          }
          if (item.type === 'agent') {
            return (
              <div key={i} style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
                <GutterRule />
                <div style={{ flex: 1 }}>
                  <AgentCard item={item} />
                </div>
              </div>
            );
          }
          if (item.type === 'meta') {
            return (
              <div
                key={i}
                style={{
                  display: 'flex', alignItems: 'center', gap: 8, padding: '4px 0 6px',
                  fontSize: 11.5, color: t.text4, marginTop: 8,
                }}
              >
                <span style={{ width: 8, height: 8, borderRadius: 99, background: `color-mix(in oklab, ${t.warn} 50%, transparent)` }} />
                <span className="mono">{item.dur}</span>
                <span>·</span>
                <span className="mono">{item.tokens}</span>
              </div>
            );
          }
          return null;
        })}

        {/* Streaming affordance — shown at the tail while a live turn runs. */}
        {running && (
          <div style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
            <GutterRule />
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, color: t.text3, fontSize: 13.5 }}>
              <span
                style={{
                  width: 10, height: 10, borderRadius: 99, background: t.accent,
                  boxShadow: `0 0 0 4px color-mix(in oklab, ${t.accent} 22%, transparent)`,
                  animation: 'shimmer 1.3s infinite', flexShrink: 0,
                }}
              />
              <span style={{ animation: 'cursor-blink 1.1s step-end infinite' }}>Thinking…</span>
            </div>
          </div>
        )}

        {/* Scroll anchor — keeps the newest content in view as deltas arrive. */}
        <div ref={tailRef} />
      </div>
    </div>
  );
}
