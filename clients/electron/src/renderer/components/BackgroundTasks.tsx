import { useState, useEffect } from 'react';
import { useT } from '../theme/ThemeContext';
import {
  BG_TASKS_RUNNING, BG_TASKS_FINISHED, TRANSCRIPT_DEMO,
  type BgTask, type TranscriptStepData,
} from '../data';
import { Icon } from './Icon';
import { iconBtn } from './primitives';

export function BackgroundTasks({ open, setOpen }: { open: boolean; setOpen: (v: boolean) => void }) {
  const t = useT();
  const [view, setView] = useState<{ name: 'list' | 'transcript'; task: BgTask | null }>({ name: 'list', task: null });

  // reset to list whenever panel re-opens
  useEffect(() => {
    if (open) setView({ name: 'list', task: null });
  }, [open]);

  if (!open) return null;

  return (
    <>
      <div onClick={() => setOpen(false)} style={{ position: 'absolute', inset: 0, zIndex: 48, background: 'transparent' }} />
      <div
        style={{
          position: 'absolute', top: 0, right: 0, bottom: 0, zIndex: 49,
          width: 440, background: t.sidebarBg,
          borderLeft: `0.5px solid ${t.border}`,
          boxShadow: '-12px 0 32px rgba(0,0,0,0.18)',
          display: 'flex', flexDirection: 'column',
          animation: 'slide-in-r 0.22s ease',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        {view.name === 'list' ? (
          <BgTasksList onClose={() => setOpen(false)} onOpenTranscript={(task) => setView({ name: 'transcript', task })} />
        ) : (
          <BgTasksTranscript task={view.task} onBack={() => setView({ name: 'list', task: null })} onClose={() => setOpen(false)} />
        )}
      </div>
    </>
  );
}

export function BgTasksList({
  onClose, onOpenTranscript, embedded,
}: {
  onClose: () => void;
  onOpenTranscript: (task: BgTask) => void;
  embedded?: boolean;
}) {
  const t = useT();
  return (
    <>
      {!embedded && (
        <div
          style={{
            padding: '12px 14px', display: 'flex', alignItems: 'center', gap: 8,
            borderBottom: `0.5px solid ${t.border}`, flexShrink: 0,
          }}
        >
          <span style={{ flex: 1, fontSize: 14, color: t.text, fontWeight: 600 }}>Background tasks</span>
          <button
            onClick={onClose}
            style={iconBtn(t)}
            onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
          >
            <Icon name="x" size={14} color={t.text3} stroke={1.8} />
          </button>
        </div>
      )}

      {/* Scroll body */}
      <div style={{ flex: 1, overflowY: 'auto', padding: 10 }}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {BG_TASKS_RUNNING.map((r) => (
            <TaskCard key={r.id} item={r} running onOpenTranscript={() => onOpenTranscript(r)} />
          ))}
        </div>

        <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '14px 4px 8px' }}>
          <span style={{ flex: 1, fontSize: 12, color: t.text3, fontWeight: 500 }}>Finished</span>
          <button
            style={{
              background: 'transparent', border: 'none', cursor: 'pointer',
              color: t.text3, fontSize: 12, fontFamily: 'inherit', padding: '2px 6px', borderRadius: 5,
            }}
            onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
          >
            Clear
          </button>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {BG_TASKS_FINISHED.map((f) => (
            <TaskCard key={f.id} item={f} onOpenTranscript={() => onOpenTranscript(f)} />
          ))}
        </div>
      </div>
    </>
  );
}

function TaskCard({ item, running, onOpenTranscript }: { item: BgTask; running?: boolean; onOpenTranscript: () => void }) {
  const t = useT();
  const isBashOnly = item.kind === 'Bash' && !item.dur;
  const linkColor = t.link || t.accent2 || t.accent;
  return (
    <div
      style={{
        padding: '10px 12px', background: t.surfaceHover, borderRadius: 8,
        display: 'flex', flexDirection: 'column', gap: 6,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'flex-start', gap: 8 }}>
        <span
          style={{
            width: 8, height: 8, borderRadius: 99, marginTop: 6, flexShrink: 0,
            background: running ? t.accent : t.text4,
            boxShadow: running ? `0 0 0 3px color-mix(in oklab, ${t.accent} 22%, transparent)` : 'none',
          }}
        />
        <span style={{ flex: 1, fontSize: 13, color: t.text, fontWeight: 600, lineHeight: 1.4 }}>{item.title}</span>
        {running && (
          <button
            style={{
              background: t.surface, border: `0.5px solid ${t.border}`,
              color: t.text2, fontSize: 11.5, fontFamily: 'inherit',
              padding: '3px 9px', borderRadius: 6, cursor: 'pointer',
            }}
            onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceActive)}
            onMouseLeave={(e) => (e.currentTarget.style.background = t.surface)}
          >
            Stop
          </button>
        )}
      </div>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, fontSize: 11.5, color: t.text3, paddingLeft: 16, flexWrap: 'wrap' }}>
        {running ? (
          <>
            <span>Running agent</span>
            <span>{item.dur}</span>
          </>
        ) : (
          <>
            <span>{item.kind}</span>
            <span>Completed</span>
            {item.dur && <span>{item.dur}</span>}
          </>
        )}
      </div>
      {!isBashOnly && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, fontSize: 11.5, color: t.text3, paddingLeft: 16, flexWrap: 'wrap' }}>
          {item.tokens && <span>{item.tokens} Tokens</span>}
          {item.tools != null && <span>{item.tools} Tool uses</span>}
          {running && item.tool && <span>{item.tool}</span>}
          {!running && (
            <a onClick={onOpenTranscript} style={{ color: linkColor, cursor: 'pointer', textDecoration: 'none', fontWeight: 500 }}>
              View transcript
            </a>
          )}
        </div>
      )}
      {running && (
        <div style={{ paddingLeft: 16 }}>
          <a onClick={onOpenTranscript} style={{ color: linkColor, fontSize: 11.5, cursor: 'pointer', textDecoration: 'none', fontWeight: 500 }}>
            View transcript
          </a>
        </div>
      )}
    </div>
  );
}

export function BgTasksTranscript({
  task, onBack, onClose, embedded,
}: {
  task: BgTask | null;
  onBack: () => void;
  onClose: () => void;
  embedded?: boolean;
}) {
  const t = useT();
  return (
    <>
      <div
        style={{
          padding: '10px 10px 10px 6px', display: 'flex', alignItems: 'center', gap: 4,
          borderBottom: `0.5px solid ${t.border}`, flexShrink: 0,
        }}
      >
        <button
          onClick={onBack}
          style={iconBtn(t)}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke={t.text2} strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round">
            <path d="m15 18-6-6 6-6" />
          </svg>
        </button>
        <span
          style={{ flex: 1, fontSize: 13.5, color: t.text, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
          title={task?.title}
        >
          {task?.title || 'Transcript'}
        </span>
        {!embedded && (
          <button
            onClick={onClose}
            style={iconBtn(t)}
            onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
          >
            <Icon name="x" size={14} color={t.text3} stroke={1.8} />
          </button>
        )}
      </div>

      {/* Scroll body */}
      <div style={{ flex: 1, overflowY: 'auto', padding: '12px 14px' }}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          {TRANSCRIPT_DEMO.map((step, i) => (
            <TranscriptStep key={i} step={step} />
          ))}
        </div>
      </div>
    </>
  );
}

function TranscriptStep({ step }: { step: TranscriptStepData }) {
  const t = useT();
  const [open, setOpen] = useState<boolean>(step.kind === 'tool' ? !!step.expanded : false);

  if (step.kind === 'tool') {
    return (
      <button
        onClick={() => setOpen((o) => !o)}
        style={{
          display: 'flex', alignItems: 'center', gap: 6,
          padding: '2px 4px', borderRadius: 6,
          border: 'none', background: 'transparent', cursor: 'pointer',
          color: t.text2, fontSize: 13.5, fontFamily: 'inherit',
          textAlign: 'left', lineHeight: 1.45,
        }}
        onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
        onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
      >
        <span style={{ color: t.text3 }}>{step.label}</span>
        {step.target && <span style={{ color: t.text, fontWeight: 600 }}>{step.target}</span>}
        <Icon name="chevron" size={12} color={t.text4} stroke={2} style={{ transform: open ? 'rotate(180deg)' : undefined }} />
      </button>
    );
  }

  // bash
  return (
    <div style={{ background: t.surface, border: `0.5px solid ${t.border}`, borderRadius: 10, overflow: 'hidden' }}>
      <div style={{ padding: '8px 12px', display: 'flex', alignItems: 'center', gap: 8, borderBottom: `0.5px solid ${t.border}` }}>
        <span style={{ flex: 1, fontSize: 12.5, color: t.text3, fontWeight: 500 }}>Bash</span>
        <button
          style={iconBtn(t)}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke={t.text3} strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
            <rect x="8" y="8" width="12" height="12" rx="2" />
            <path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
          </svg>
        </button>
      </div>
      <div className="mono" style={{ padding: '10px 12px', fontSize: 12, lineHeight: 1.55, color: t.text2, wordBreak: 'break-all' }}>
        <div style={{ color: t.add }}>
          <span style={{ color: t.text3, marginRight: 6 }}>$</span>
          {step.cmd}
        </div>
        <div style={{ height: 8 }} />
        {step.out.map((line, i) => (
          <div key={i} style={{ color: t.text2 }}>
            {line}
          </div>
        ))}
      </div>
    </div>
  );
}
