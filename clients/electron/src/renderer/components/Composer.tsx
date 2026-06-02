import { useState, useEffect, useRef } from 'react';
import { useT } from '../theme/ThemeContext';
import { SLASH_COMMANDS, type Model, type Project, type RunItem } from '../data';
import { Icon } from './Icon';
import { iconBtn } from './primitives';
import { PermissionPicker, ModelPicker, ContextPicker } from './pickers';

interface ComposerProps {
  repo: Project;
  model: Model;
  setModel: (m: Model) => void;
  appendMessage: (msg: RunItem) => void;
  /** Submit a text prompt to the live engine (no-op-able in browser preview). */
  onSubmit?: (text: string) => void;
  /** Cancel the in-flight live turn. */
  onCancel?: () => void;
  /** True while a live turn is streaming — shows the thinking affordance. */
  running?: boolean;
}

const NUM_BARS = 90;

export function Composer({ repo, model, setModel, appendMessage, onSubmit, onCancel, running = false }: ComposerProps) {
  const t = useT();
  const [text, setText] = useState('');
  const [slashOpen, setSlashOpen] = useState(false);
  const [modelOpen, setModelOpen] = useState(false);
  const [contextOpen, setContextOpen] = useState(false);
  const [permMode, setPermMode] = useState('bypass');
  const [permOpen, setPermOpen] = useState(false);
  const [effort, setEffort] = useState('max');
  const [fastMode, setFastMode] = useState(true);
  const taRef = useRef<HTMLTextAreaElement>(null);

  // ─── Audio recording ─────────────────────────────────────
  const [recording, setRecording] = useState(false);
  const [recSecs, setRecSecs] = useState(0);
  const [micDenied, setMicDenied] = useState(false);
  const waveCanvasRef = useRef<HTMLCanvasElement>(null);
  const audioCtxRef = useRef<AudioContext | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const rafRef = useRef<number | null>(null);
  const barsRef = useRef<number[]>([]);

  useEffect(() => {
    if (!recording) return;
    barsRef.current = new Array<number>(NUM_BARS).fill(0);
    const startTime = performance.now();
    let cancelled = false;

    void (async () => {
      let stream: MediaStream;
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      } catch {
        if (!cancelled) {
          setMicDenied(true);
          setRecording(false);
        }
        return;
      }
      if (cancelled) {
        stream.getTracks().forEach((tr) => tr.stop());
        return;
      }
      streamRef.current = stream;
      const AC = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
      const audioCtx = new AC();
      audioCtxRef.current = audioCtx;
      const source = audioCtx.createMediaStreamSource(stream);
      const analyser = audioCtx.createAnalyser();
      analyser.fftSize = 1024;
      analyser.smoothingTimeConstant = 0.4;
      source.connect(analyser);
      const buffer = new Uint8Array(analyser.fftSize);

      const drawWaveform = () => {
        const canvas = waveCanvasRef.current;
        if (!canvas) return;
        const dpr = window.devicePixelRatio || 1;
        const rect = canvas.getBoundingClientRect();
        if (canvas.width !== Math.round(rect.width * dpr) || canvas.height !== Math.round(rect.height * dpr)) {
          canvas.width = Math.round(rect.width * dpr);
          canvas.height = Math.round(rect.height * dpr);
        }
        const ctx2d = canvas.getContext('2d');
        if (!ctx2d) return;
        ctx2d.setTransform(dpr, 0, 0, dpr, 0, 0);
        ctx2d.clearRect(0, 0, rect.width, rect.height);

        const cy = rect.height / 2;
        const dotColor = t.text4;
        const barColor = t.text;

        ctx2d.fillStyle = dotColor;
        for (let x = 0; x < rect.width; x += 5) {
          ctx2d.fillRect(x, cy - 0.5, 1.5, 1);
        }

        const bars = barsRef.current;
        const barW = 1.8;
        const gap = 2.4;
        const slot = barW + gap;
        const maxBarsThatFit = Math.floor(rect.width / slot);
        const visible = Math.min(bars.length, maxBarsThatFit);
        ctx2d.fillStyle = barColor;
        for (let i = bars.length - visible; i < bars.length; i++) {
          const v = bars[i];
          if (v <= 0.002) continue;
          const h = Math.max(1.5, Math.min(rect.height * 0.92, v * rect.height * 3.6));
          const x = rect.width - (bars.length - i) * slot;
          ctx2d.fillRect(x, cy - h / 2, barW, h);
        }
      };

      const tick = () => {
        if (cancelled) return;
        analyser.getByteTimeDomainData(buffer);
        let sum = 0;
        for (let i = 0; i < buffer.length; i++) {
          const v = (buffer[i] - 128) / 128;
          sum += v * v;
        }
        const rms = Math.sqrt(sum / buffer.length);
        barsRef.current.shift();
        barsRef.current.push(rms);
        drawWaveform();
        const elapsed = Math.floor((performance.now() - startTime) / 1000);
        setRecSecs(elapsed);
        rafRef.current = requestAnimationFrame(tick);
      };
      rafRef.current = requestAnimationFrame(tick);
    })();

    return () => {
      cancelled = true;
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
      if (streamRef.current) {
        streamRef.current.getTracks().forEach((tr) => tr.stop());
        streamRef.current = null;
      }
      if (audioCtxRef.current) {
        try {
          void audioCtxRef.current.close();
        } catch {
          /* noop */
        }
        audioCtxRef.current = null;
      }
      setRecSecs(0);
    };
  }, [recording, t.text, t.text4]);

  const fmtTime = (s: number) => {
    const m = Math.floor(s / 60);
    const sec = (s % 60).toString().padStart(2, '0');
    return `${m}:${sec}`;
  };

  useEffect(() => {
    if (text.startsWith('/')) setSlashOpen(true);
    else setSlashOpen(false);
  }, [text]);

  const filtered = SLASH_COMMANDS.filter((c) => c.cmd.startsWith(text || '/'));

  // Submit the composed prompt to the live engine, then clear + reset the box.
  const submit = () => {
    const value = text.trim();
    if (!value) return;
    onSubmit?.(value);
    setText('');
    setSlashOpen(false);
    const ta = taRef.current;
    if (ta) ta.style.height = 'auto';
  };

  return (
    <div style={{ padding: '0 32px 16px', flexShrink: 0, background: t.stageBg, borderTop: `0.5px solid ${t.border}` }}>
      <div style={{ maxWidth: 920, margin: '0 auto' }}>
        {/* Diff/commit bar */}
        {repo.diff && (
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '10px 4px 12px' }}>
            <div
              style={{
                display: 'flex', alignItems: 'center', gap: 8,
                padding: '6px 12px', borderRadius: 8,
                background: t.surface, border: `0.5px solid ${t.border}`,
              }}
            >
              <span style={{ width: 6, height: 6, borderRadius: 99, background: t.warn }} />
              <span style={{ fontSize: 12.5, color: t.text, fontWeight: 500 }}>{repo.name}</span>
              <span className="mono" style={{ fontSize: 12, color: t.text3 }}>{repo.branch}</span>
            </div>
            <div style={{ flex: 1 }} />
            <div className="mono" style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 13, fontWeight: 600 }}>
              <span style={{ color: t.add }}>+{repo.diff.add}</span>
              <span style={{ color: t.text4 }}>−{repo.diff.del}</span>
            </div>
            <button
              style={{
                padding: '7px 14px', borderRadius: 8,
                background: 'transparent', color: t.text3,
                border: `0.5px solid ${t.border}`, cursor: 'not-allowed',
                fontSize: 12.5, fontWeight: 600, fontFamily: 'inherit',
              }}
            >
              Commit changes
            </button>
          </div>
        )}

        {/* Slash menu */}
        {slashOpen && filtered.length > 0 && (
          <div
            style={{
              background: t.surface, border: `0.5px solid ${t.borderStrong}`,
              borderRadius: 10, padding: 4, marginBottom: 6,
              boxShadow: '0 12px 32px rgba(0,0,0,0.25)',
              animation: 'fade-in 0.15s ease', maxHeight: 280, overflowY: 'auto',
            }}
          >
            {filtered.map((c) => (
              <div
                key={c.cmd}
                onMouseDown={(e) => {
                  e.preventDefault();
                  setText(c.cmd + ' ');
                  setSlashOpen(false);
                  taRef.current?.focus();
                }}
                style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '8px 10px', borderRadius: 7, cursor: 'pointer' }}
                onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
                onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
              >
                <span
                  className="mono"
                  style={{
                    fontSize: 12.5, color: t.accent, fontWeight: 600,
                    background: t.accentBg, padding: '2px 7px', borderRadius: 5,
                    border: `0.5px solid ${t.accentBorder}`, minWidth: 80,
                  }}
                >
                  {c.cmd}
                </span>
                <span style={{ flex: 1, fontSize: 12.5, color: t.text2 }}>{c.desc}</span>
                {c.hint && <span style={{ fontSize: 10.5, color: t.text4 }}>{c.hint}</span>}
                {c.sub && <span style={{ fontSize: 10.5, color: t.text4 }}>{c.sub}</span>}
              </div>
            ))}
          </div>
        )}

        {/* Composer */}
        <div style={{ background: t.surface, border: `0.5px solid ${t.borderStrong}`, borderRadius: 12, padding: '10px 12px 8px' }}>
          <textarea
            ref={taRef}
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              e.target.style.height = 'auto';
              e.target.style.height = Math.min(e.target.scrollHeight, 220) + 'px';
            }}
            onKeyDown={(e) => {
              // Enter submits; Shift+Enter (and IME composition) inserts a newline.
              if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                submit();
              }
            }}
            placeholder={recording ? 'Ask for follow-up changes' : 'Type / for commands'}
            disabled={recording}
            rows={1}
            style={{
              width: '100%', border: 'none', outline: 'none', resize: 'none',
              background: 'transparent', color: t.text, fontSize: 14.5,
              lineHeight: 1.55, fontFamily: 'inherit',
              minHeight: 22, maxHeight: 220, opacity: recording ? 0.55 : 1,
            }}
          />
          {recording ? (
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 6 }}>
              <button
                title="附加上下文"
                style={iconBtn(t)}
                onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
                onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
              >
                <Icon name="plus" size={14} color={t.text3} stroke={2} />
              </button>
              <canvas ref={waveCanvasRef} style={{ flex: 1, height: 28, minWidth: 0, display: 'block' }} />
              <span
                className="mono"
                style={{ fontSize: 12.5, color: t.text2, fontVariantNumeric: 'tabular-nums', minWidth: 30, textAlign: 'right' }}
              >
                {fmtTime(recSecs)}
              </span>
              <button
                title="停止录音"
                onClick={() => setRecording(false)}
                style={{
                  width: 28, height: 28, borderRadius: 99, border: 'none', cursor: 'pointer',
                  background: t.surfaceActive, color: t.text2,
                  display: 'flex', alignItems: 'center', justifyContent: 'center',
                }}
                onMouseEnter={(e) => (e.currentTarget.style.filter = 'brightness(0.95)')}
                onMouseLeave={(e) => (e.currentTarget.style.filter = 'none')}
              >
                <span style={{ width: 9, height: 9, borderRadius: 1.5, background: t.text }} />
              </button>
              <button
                title="发送"
                onClick={() => {
                  const bars = (barsRef.current || []).slice();
                  const duration = Math.max(1, recSecs);
                  appendMessage({ type: 'audio', bars, duration });
                  setRecording(false);
                }}
                style={{
                  width: 28, height: 28, borderRadius: 99, border: 'none', cursor: 'pointer',
                  background: t.text, color: t.windowBg,
                  display: 'flex', alignItems: 'center', justifyContent: 'center',
                }}
                onMouseEnter={(e) => (e.currentTarget.style.filter = 'brightness(1.1)')}
                onMouseLeave={(e) => (e.currentTarget.style.filter = 'none')}
              >
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke={t.windowBg} strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M12 19V5M5 12l7-7 7 7" />
                </svg>
              </button>
            </div>
          ) : (
            <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginTop: 6, position: 'relative' }}>
              <PermissionPicker mode={permMode} setMode={setPermMode} open={permOpen} setOpen={setPermOpen} />
              <button
                title="附加上下文"
                style={iconBtn(t)}
                onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
                onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
              >
                <Icon name="plus" size={14} color={t.text3} stroke={2} />
              </button>
              <button
                title={micDenied ? '麦克风权限被拒' : '语音'}
                onClick={() => {
                  setMicDenied(false);
                  setRecording(true);
                }}
                style={iconBtn(t)}
                onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
                onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
              >
                <Icon name="mic" size={13} color={micDenied ? t.danger : t.text3} stroke={1.8} />
              </button>
              <button
                title="语音选项"
                style={{ ...iconBtn(t), width: 18 }}
                onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
                onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
              >
                <Icon name="chevron" size={11} color={t.text3} stroke={2} />
              </button>

              <div style={{ flex: 1 }} />

              {running ? (
                <button
                  title="停止"
                  onClick={() => onCancel?.()}
                  style={{ ...iconBtn(t), width: 28, height: 28, background: t.surfaceHover, color: t.text2 }}
                  onMouseEnter={(e) => (e.currentTarget.style.filter = 'brightness(0.95)')}
                  onMouseLeave={(e) => (e.currentTarget.style.filter = 'none')}
                >
                  <span style={{ width: 10, height: 10, borderRadius: 2, background: t.text2 }} />
                </button>
              ) : (
                <button
                  title="发送"
                  onClick={() => submit()}
                  disabled={text.trim().length === 0}
                  style={{
                    width: 28, height: 28, borderRadius: 99, border: 'none',
                    cursor: text.trim().length === 0 ? 'default' : 'pointer',
                    background: text.trim().length === 0 ? t.surfaceHover : t.text,
                    color: text.trim().length === 0 ? t.text4 : t.windowBg,
                    display: 'flex', alignItems: 'center', justifyContent: 'center',
                    transition: 'background 0.15s ease',
                  }}
                  onMouseEnter={(e) => {
                    if (text.trim().length > 0) e.currentTarget.style.filter = 'brightness(1.1)';
                  }}
                  onMouseLeave={(e) => (e.currentTarget.style.filter = 'none')}
                >
                  <svg
                    width="14" height="14" viewBox="0 0 24 24" fill="none"
                    stroke={text.trim().length === 0 ? t.text4 : t.windowBg}
                    strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"
                  >
                    <path d="M12 19V5M5 12l7-7 7 7" />
                  </svg>
                </button>
              )}
            </div>
          )}
        </div>

        {/* Footer status row */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '8px 4px 0', fontSize: 11.5, color: t.text4 }}>
          <span style={{ flex: 1 }} />
          <ModelPicker
            model={model}
            setModel={setModel}
            open={modelOpen}
            setOpen={setModelOpen}
            effort={effort}
            setEffort={setEffort}
            fastMode={fastMode}
            setFastMode={setFastMode}
          />
          <ContextPicker open={contextOpen} setOpen={setContextOpen} />
        </div>
      </div>
    </div>
  );
}
