import { useState, useEffect, useRef, useMemo } from 'react';
import { useT } from '../theme/ThemeContext';
import { RUN, type RunItem } from '../data';
import { Icon } from './Icon';

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
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, position: 'relative' }}>
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
              background: t.text3, color: t.windowBg,
            }}
          >
            <Icon name="check" size={11} stroke={3} />
          </span>
        )}
        <span style={{ fontSize: 14, fontWeight: 500, color: t.text }}>{item.title}</span>
        {item.expandable && <Icon name="chevronR" size={13} color={t.text3} stroke={2} />}
      </div>
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
    </div>
  );
}

function NarrationLine({ item }: { item: Extract<RunItem, { type: 'narration' }> }) {
  const t = useT();
  const color = item.tone === 'muted' ? t.text3 : t.text;
  // tokenize check/x emojis
  const parts = item.text.split(/(✓|✗)/g);
  return (
    <div style={{ fontSize: 14.5, lineHeight: 1.7, color, fontWeight: item.strong ? 500 : 400, maxWidth: 880 }}>
      {parts.map((p, i) => {
        if (p === '✓') {
          return (
            <span
              key={i}
              style={{
                display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                width: 16, height: 16, borderRadius: 4, background: t.ok, color: '#fff',
                verticalAlign: -3, margin: '0 1px',
              }}
            >
              <Icon name="check" size={11} stroke={3} color="#fff" />
            </span>
          );
        }
        if (p === '✗') {
          return (
            <span
              key={i}
              style={{
                display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                width: 16, height: 16, borderRadius: 4, background: t.danger, color: '#fff',
                verticalAlign: -3, margin: '0 1px',
              }}
            >
              <Icon name="x" size={11} stroke={3} color="#fff" />
            </span>
          );
        }
        return <span key={i}>{p}</span>;
      })}
    </div>
  );
}

// ─── AUDIO MESSAGE (user voice message bubble with static waveform) ─
function AudioMessage({ bars, duration }: { bars: number[]; duration: number }) {
  const t = useT();
  const [playing, setPlaying] = useState(false);
  const [progress, setProgress] = useState(0); // 0..1
  const rafRef = useRef<number | null>(null);
  const startRef = useRef(0);

  useEffect(() => {
    if (!playing) return;
    startRef.current = performance.now() - progress * duration * 1000;
    const tick = () => {
      const elapsed = (performance.now() - startRef.current) / 1000;
      const p = Math.min(1, elapsed / duration);
      setProgress(p);
      if (p >= 1) {
        setPlaying(false);
        setProgress(0);
        return;
      }
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);
    return () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing, duration]);

  const fmt = (s: number) => {
    const m = Math.floor(s / 60);
    const sec = Math.floor(s % 60).toString().padStart(2, '0');
    return `${m}:${sec}`;
  };

  // Downsample bars to a tidy fixed count for display
  const N = 56;
  const display = useMemo(() => {
    if (!bars || bars.length === 0) return new Array<number>(N).fill(0);
    const out = new Array<number>(N).fill(0);
    const step = bars.length / N;
    for (let i = 0; i < N; i++) {
      const lo = Math.floor(i * step);
      const hi = Math.max(lo + 1, Math.floor((i + 1) * step));
      let max = 0;
      for (let j = lo; j < hi && j < bars.length; j++) max = Math.max(max, bars[j]);
      out[i] = max;
    }
    return out;
  }, [bars]);

  const W = 220;
  const H = 28;
  const slot = W / N;
  const barW = 1.8;
  const played = Math.floor(progress * N);

  return (
    <div
      style={{
        display: 'inline-flex', alignItems: 'center', gap: 12,
        padding: '8px 12px 8px 8px', borderRadius: 18,
        background: t.accentBg, border: `0.5px solid ${t.accentBorder}`, maxWidth: 360,
      }}
    >
      <button
        onClick={() => setPlaying((p) => !p)}
        style={{
          width: 30, height: 30, borderRadius: 99, border: 'none', cursor: 'pointer',
          background: t.accent, color: '#fff',
          display: 'flex', alignItems: 'center', justifyContent: 'center', flexShrink: 0,
        }}
        onMouseEnter={(e) => (e.currentTarget.style.filter = 'brightness(1.08)')}
        onMouseLeave={(e) => (e.currentTarget.style.filter = 'none')}
      >
        {playing ? (
          <svg width="11" height="11" viewBox="0 0 24 24" fill="#fff">
            <rect x="6" y="5" width="4" height="14" rx="1" />
            <rect x="14" y="5" width="4" height="14" rx="1" />
          </svg>
        ) : (
          <svg width="11" height="11" viewBox="0 0 24 24" fill="#fff">
            <path d="M7 4l13 8-13 8z" />
          </svg>
        )}
      </button>
      <svg width={W} height={H} viewBox={`0 0 ${W} ${H}`} style={{ display: 'block', flexShrink: 0 }}>
        {/* dotted baseline */}
        {Array.from({ length: Math.floor(W / 5) }).map((_, i) => (
          <rect key={`d${i}`} x={i * 5} y={H / 2 - 0.5} width={1.5} height={1} fill={t.text4} opacity={0.55} />
        ))}
        {/* bars */}
        {display.map((v, i) => {
          if (v <= 0.003) return null;
          const h = Math.max(2, Math.min(H * 0.92, v * H * 3.6));
          const x = i * slot + (slot - barW) / 2;
          const active = i < played;
          return (
            <rect
              key={i}
              x={x}
              y={H / 2 - h / 2}
              width={barW}
              height={h}
              rx={0.6}
              fill={active ? t.accent2 || t.accent : t.text}
              opacity={active ? 1 : 0.85}
            />
          );
        })}
      </svg>
      <span
        className="mono"
        style={{ fontSize: 12, color: t.text2, fontVariantNumeric: 'tabular-nums', minWidth: 30, textAlign: 'right' }}
      >
        {fmt(duration)}
      </span>
    </div>
  );
}

// ─── STAGE (the agent run scrollback) ────────────────────────
export function Stage({ extraMessages = [] }: { extraMessages?: RunItem[] }) {
  const t = useT();
  return (
    <div style={{ flex: 1, overflowY: 'auto', background: t.stageBg, position: 'relative' }}>
      <div
        style={{
          maxWidth: 920, margin: '0 auto',
          padding: '28px 32px 12px',
          display: 'flex', flexDirection: 'column', gap: 18,
        }}
      >
        {[...RUN, ...extraMessages].map((item, i) => {
          if (item.type === 'narration') {
            return (
              <div key={i} style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
                <GutterRule />
                <NarrationLine item={item} />
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
          if (item.type === 'audio') {
            return (
              <div key={i} style={{ display: 'flex', justifyContent: 'flex-end', animation: 'fade-in 0.3s ease' }}>
                <AudioMessage bars={item.bars} duration={item.duration} />
              </div>
            );
          }
          return null;
        })}

        {/* /compact pill — pre-input action chip */}
        <div style={{ marginTop: 4 }}>
          <span
            className="mono"
            style={{
              display: 'inline-flex', alignItems: 'center', gap: 6,
              padding: '4px 9px', borderRadius: 99,
              background: t.accentBg, border: `0.5px solid ${t.accentBorder}`,
              color: t.accent, fontSize: 12, fontWeight: 500,
            }}
          >
            /compact
          </span>
        </div>
      </div>
    </div>
  );
}
