import { useState, useMemo } from 'react';
import { useT } from '../theme/ThemeContext';
import { FILES_CHANGED, BG_TASKS_RUNNING, type BgTask } from '../data';
import { Icon } from './Icon';
import { iconBtn } from './primitives';
import { BgTasksList, BgTasksTranscript } from './BackgroundTasks';

interface DiffLine {
  t: 'meta' | 'add' | 'del' | 'ctx';
  n: number | string;
  text: string;
}

export function RightPanel({ onClose }: { onClose: () => void }) {
  const t = useT();
  const [tab, setTab] = useState<'files' | 'plan' | 'tasks' | 'terminal'>('files');
  const [activeFile, setActiveFile] = useState(FILES_CHANGED[5].path);
  const [tasksView, setTasksView] = useState<{ name: 'list' | 'transcript'; task: BgTask | null }>({ name: 'list', task: null });

  // Simple synthetic diff for the active file
  const diffLines = useMemo<DiffLine[]>(() => {
    const f = FILES_CHANGED.find((x) => x.path === activeFile);
    if (!f) return [];
    const base: DiffLine[] = [
      { t: 'meta', n: '', text: `@@ ${f.status === 'A' ? '0,0 +1,' + Math.min(f.add, 40) : '1,3 +1,' + (f.add + 3)} @@` },
    ];
    const lines: DiffLine[] = [];
    if (activeFile.endsWith('.toml')) {
      lines.push(
        { t: 'add', n: 1, text: '[package]' },
        { t: 'add', n: 2, text: 'name = "lingxi-protocol"' },
        { t: 'add', n: 3, text: 'version = "0.1.0"' },
        { t: 'add', n: 4, text: 'edition = "2021"' },
        { t: 'add', n: 5, text: '' },
        { t: 'add', n: 6, text: '[dependencies]' },
        { t: 'add', n: 7, text: 'serde = { version = "1", features = ["derive"] }' },
        { t: 'add', n: 8, text: 'serde_json = "1"' },
        { t: 'add', n: 9, text: 'thiserror = "1"' },
        { t: 'add', n: 10, text: 'bytes = "1.6"' },
      );
    } else if (activeFile.endsWith('.rs')) {
      lines.push(
        { t: 'add', n: 1, text: '//! Wire protocol for Lingxi agents.' },
        { t: 'add', n: 2, text: '' },
        { t: 'add', n: 3, text: 'use serde::{Deserialize, Serialize};' },
        { t: 'add', n: 4, text: '' },
        { t: 'add', n: 5, text: '#[derive(Debug, Clone, Serialize, Deserialize)]' },
        { t: 'add', n: 6, text: 'pub struct AgentId(pub u64);' },
        { t: 'add', n: 7, text: '' },
        { t: 'add', n: 8, text: '#[derive(Debug, Clone, Serialize, Deserialize)]' },
        { t: 'add', n: 9, text: 'pub struct TaskId(pub u64);' },
        { t: 'add', n: 10, text: '' },
        { t: 'add', n: 11, text: 'impl AgentId {' },
        { t: 'add', n: 12, text: '    pub fn new() -> Self { Self(0) }' },
        { t: 'add', n: 13, text: '}' },
      );
    } else {
      lines.push(
        { t: 'ctx', n: 1, text: '# ' + f.path },
        { t: 'add', n: 2, text: '' },
        { t: 'add', n: 3, text: '## Lingxi-Next workspace' },
        { t: 'add', n: 4, text: '' },
        { t: 'add', n: 5, text: 'Rust workspace with 5 crates:' },
        { t: 'add', n: 6, text: '- lingxi-core' },
        { t: 'add', n: 7, text: '- lingxi-protocol' },
        { t: 'add', n: 8, text: '- lingxi-store' },
      );
    }
    return [...base, ...lines];
  }, [activeFile]);

  const tabs: { id: 'files' | 'plan' | 'tasks' | 'terminal'; icon: string; label: string; count?: number }[] = [
    { id: 'files', icon: 'file', label: 'Diff', count: FILES_CHANGED.length },
    { id: 'plan', icon: 'box', label: 'Plan' },
    { id: 'tasks', icon: 'tasks', label: 'Tasks', count: BG_TASKS_RUNNING.length },
    { id: 'terminal', icon: 'terminal', label: 'Shell' },
  ];

  const planSteps: { i: number; t: string; s: 'done' | 'running' | 'todo'; d?: string; dur?: string }[] = [
    { i: 1, t: 'Bootstrap workspace', s: 'done', d: 'Cargo.toml + readme', dur: '2m 14s' },
    { i: 2, t: 'Create lingxi-protocol skeleton', s: 'done', d: '5 crates · 11 files', dur: '4m 02s' },
    { i: 3, t: 'IDs (newtype wrappers)', s: 'running', d: 'AgentId · TaskId · MessageId', dur: '38s' },
    { i: 4, t: 'Codec — bincode + serde', s: 'todo' },
    { i: 5, t: 'Async runtime trait', s: 'todo' },
    { i: 6, t: 'CLI entrypoint', s: 'todo' },
    { i: 7, t: 'Integration tests', s: 'todo' },
  ];

  return (
    <div
      style={{
        width: 380, flexShrink: 0, background: t.sidebarBg,
        borderLeft: `0.5px solid ${t.border}`,
        display: 'flex', flexDirection: 'column',
        animation: 'slide-in-r 0.18s ease',
      }}
    >
      {/* Tab header */}
      <div style={{ height: 44, flexShrink: 0, padding: '0 8px 0 14px', display: 'flex', alignItems: 'center', gap: 2, borderBottom: `0.5px solid ${t.border}` }}>
        {tabs.map((x) => {
          const active = tab === x.id;
          return (
            <button
              key={x.id}
              onClick={() => setTab(x.id)}
              style={{
                display: 'flex', alignItems: 'center', gap: 6,
                padding: '6px 10px', borderRadius: 7, border: 'none', cursor: 'pointer',
                background: active ? t.surfaceActive : 'transparent',
                color: active ? t.text : t.text3,
                fontSize: 12, fontWeight: active ? 600 : 500, fontFamily: 'inherit',
              }}
              onMouseEnter={(e) => {
                if (!active) e.currentTarget.style.background = t.surfaceHover;
              }}
              onMouseLeave={(e) => {
                if (!active) e.currentTarget.style.background = 'transparent';
              }}
            >
              <Icon name={x.icon} size={13} stroke={1.8} />
              {x.label}
              {x.count !== undefined && (
                <span style={{ fontSize: 10, color: active ? t.accent : t.text4, fontWeight: 700, marginLeft: 1 }}>{x.count}</span>
              )}
            </button>
          );
        })}
        <div style={{ flex: 1 }} />
        <button
          onClick={onClose}
          style={iconBtn(t)}
          onMouseEnter={(e) => (e.currentTarget.style.background = t.surfaceHover)}
          onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
        >
          <Icon name="x" size={14} color={t.text3} stroke={2} />
        </button>
      </div>

      {tab === 'files' && (
        <>
          {/* File list */}
          <div style={{ flexShrink: 0, maxHeight: 260, overflowY: 'auto', padding: '6px 6px', borderBottom: `0.5px solid ${t.border}` }}>
            {FILES_CHANGED.map((f) => {
              const active = f.path === activeFile;
              return (
                <div
                  key={f.path}
                  onClick={() => setActiveFile(f.path)}
                  style={{
                    display: 'flex', alignItems: 'center', gap: 8,
                    padding: '5px 8px', borderRadius: 6, cursor: 'pointer',
                    background: active ? t.surfaceActive : 'transparent',
                  }}
                  onMouseEnter={(e) => {
                    if (!active) e.currentTarget.style.background = t.surfaceHover;
                  }}
                  onMouseLeave={(e) => {
                    if (!active) e.currentTarget.style.background = 'transparent';
                  }}
                >
                  <span
                    style={{
                      width: 16, height: 16, borderRadius: 3, flexShrink: 0,
                      display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                      fontSize: 9.5, fontWeight: 700, fontFamily: 'inherit',
                      background:
                        f.status === 'A'
                          ? `color-mix(in oklab, ${t.add} 22%, transparent)`
                          : f.status === 'M'
                            ? `color-mix(in oklab, ${t.accent} 22%, transparent)`
                            : `color-mix(in oklab, ${t.del} 22%, transparent)`,
                      color: f.status === 'A' ? t.add : f.status === 'M' ? t.accent : t.del,
                    }}
                  >
                    {f.status}
                  </span>
                  <span
                    className="mono"
                    style={{
                      flex: 1, fontSize: 11.5, color: active ? t.text : t.text2,
                      overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                      direction: 'rtl', textAlign: 'left',
                    }}
                  >
                    {f.path}
                  </span>
                  <span className="mono" style={{ fontSize: 10.5, color: t.add, fontWeight: 600 }}>+{f.add}</span>
                </div>
              );
            })}
          </div>

          {/* Diff body */}
          <div style={{ flex: 1, overflow: 'auto', padding: '8px 0', background: t.windowBg }}>
            <div style={{ padding: '4px 14px 10px', borderBottom: `0.5px solid ${t.border}`, marginBottom: 6 }}>
              <div className="mono" style={{ fontSize: 11.5, color: t.text2, fontWeight: 600 }}>{activeFile}</div>
            </div>
            <div className="mono" style={{ fontSize: 11.5, lineHeight: 1.55, padding: '0 0 8px' }}>
              {diffLines.map((l, i) => {
                if (l.t === 'meta') {
                  return (
                    <div key={i} style={{ padding: '4px 14px', fontSize: 10.5, color: t.text4, background: t.surfaceHover, marginBottom: 4 }}>
                      {l.text}
                    </div>
                  );
                }
                const bg =
                  l.t === 'add'
                    ? `color-mix(in oklab, ${t.add} 8%, transparent)`
                    : l.t === 'del'
                      ? `color-mix(in oklab, ${t.del} 10%, transparent)`
                      : 'transparent';
                const sigil = l.t === 'add' ? '+' : l.t === 'del' ? '−' : ' ';
                const sigilColor = l.t === 'add' ? t.add : l.t === 'del' ? t.del : t.text4;
                return (
                  <div key={i} style={{ display: 'flex', background: bg }}>
                    <span style={{ width: 38, textAlign: 'right', color: t.text4, padding: '0 8px', flexShrink: 0, userSelect: 'none' }}>{l.n}</span>
                    <span style={{ width: 16, color: sigilColor, flexShrink: 0, fontWeight: 600 }}>{sigil}</span>
                    <span style={{ color: l.t === 'ctx' ? t.text3 : t.text, paddingRight: 14, whiteSpace: 'pre' }}>{l.text}</span>
                  </div>
                );
              })}
            </div>
          </div>
        </>
      )}

      {tab === 'plan' && (
        <div style={{ flex: 1, overflowY: 'auto', padding: 16 }}>
          <div style={{ fontSize: 11, color: t.text4, fontWeight: 600, letterSpacing: 0.6, textTransform: 'uppercase', marginBottom: 10 }}>
            Implementation plan
          </div>
          {planSteps.map((s) => (
            <div key={s.i} style={{ display: 'flex', gap: 12, padding: '10px 0', borderBottom: `0.5px solid ${t.border}` }}>
              <div
                style={{
                  width: 22, height: 22, borderRadius: '50%', flexShrink: 0,
                  display: 'flex', alignItems: 'center', justifyContent: 'center', marginTop: 1,
                  background:
                    s.s === 'done'
                      ? `color-mix(in oklab, ${t.ok} 22%, transparent)`
                      : s.s === 'running'
                        ? t.accentBg
                        : 'transparent',
                  border: `1px solid ${
                    s.s === 'done'
                      ? `color-mix(in oklab, ${t.ok} 35%, transparent)`
                      : s.s === 'running'
                        ? t.accentBorder
                        : t.border
                  }`,
                }}
              >
                {s.s === 'done' && <Icon name="check" size={11} color={t.ok} stroke={2.5} />}
                {s.s === 'running' && <span style={{ width: 6, height: 6, borderRadius: 99, background: t.accent, animation: 'shimmer 1.3s infinite' }} />}
                {s.s === 'todo' && <span style={{ fontSize: 10, color: t.text4, fontWeight: 500 }}>{s.i}</span>}
              </div>
              <div style={{ flex: 1 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                  <span style={{ fontSize: 13, color: s.s === 'todo' ? t.text3 : t.text, fontWeight: 500 }}>{s.t}</span>
                  {s.dur && <span className="mono" style={{ fontSize: 10.5, color: t.text4, marginLeft: 'auto' }}>{s.dur}</span>}
                </div>
                {s.d && <div style={{ fontSize: 11.5, color: t.text3, marginTop: 3, lineHeight: 1.45 }}>{s.d}</div>}
              </div>
            </div>
          ))}
        </div>
      )}

      {tab === 'tasks' &&
        (tasksView.name === 'list' ? (
          <BgTasksList embedded onClose={onClose} onOpenTranscript={(task) => setTasksView({ name: 'transcript', task })} />
        ) : (
          <BgTasksTranscript embedded task={tasksView.task} onBack={() => setTasksView({ name: 'list', task: null })} onClose={onClose} />
        ))}

      {tab === 'terminal' && (
        <div className="mono" style={{ flex: 1, overflowY: 'auto', padding: 14, background: t.windowBg }}>
          <div style={{ fontSize: 11.5, color: t.text3, lineHeight: 1.7 }}>
            <div>
              <span style={{ color: t.accent }}>~/lingxi-next</span> <span style={{ color: t.text4 }}>on</span> <span style={{ color: t.warn }}>main *</span>
            </div>
            <div>
              <span style={{ color: t.add }}>$</span> <span style={{ color: t.text }}>cargo check --workspace</span>
            </div>
            <div style={{ color: t.text2 }}>    Checking lingxi-core v0.1.0</div>
            <div style={{ color: t.text2 }}>    Checking lingxi-protocol v0.1.0</div>
            <div style={{ color: t.text2 }}>    Checking lingxi-store v0.1.0</div>
            <div style={{ color: t.text2 }}>    Checking lingxi-runtime v0.1.0</div>
            <div style={{ color: t.add }}>    Finished `dev` profile [unoptimized] in 4.82s</div>
            <div style={{ marginTop: 8 }}>
              <span style={{ color: t.accent }}>~/lingxi-next</span> <span style={{ color: t.text4 }}>on</span> <span style={{ color: t.warn }}>main *</span>
            </div>
            <div>
              <span style={{ color: t.add }}>$</span>
              <span style={{ display: 'inline-block', width: 8, height: 13, background: t.text, marginLeft: 6, verticalAlign: -2, animation: 'cursor-blink 1s infinite' }} />
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
