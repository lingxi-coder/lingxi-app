import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
import type { SessionRowDto } from '@lingxi/bridge-client';

import type { UseBridge } from '../bridge/useBridge';
import { orderedTasks } from '../bridge/desktopState';
import { engineLaunchStatus } from '../bridge/engineStatus';
import { classifyDesktopError } from '../bridge/errors';
import { useT } from '../theme/ThemeContext';
import type { ThemeMode } from '../theme/tokens';
import { Icon } from './Icon';
import { PROVIDERS, providerById } from '../../shared/providers';

function basename(path?: string): string {
  if (!path) return 'No workspace';
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

function invoke(action: () => Promise<unknown>): void {
  void action().catch(() => undefined);
}

function Button({ children, onClick, disabled = false, primary = false, success = false, danger = false, title }: {
  children: ReactNode;
  onClick(): void;
  disabled?: boolean;
  primary?: boolean;
  success?: boolean;
  danger?: boolean;
  title?: string;
}) {
  const t = useT();
  return (
    <button
      type="button"
      title={title}
      disabled={disabled}
      onClick={onClick}
      style={{
        display: 'inline-flex', alignItems: 'center', justifyContent: 'center', gap: 7,
        minHeight: 32, padding: '6px 11px', borderRadius: 8,
        border: `0.5px solid ${primary ? t.accentBorder : success ? t.ok : t.border}`,
        background: primary ? t.accent : success ? t.ok : 'transparent',
        color: danger ? t.danger : primary || success ? '#fff' : t.text2,
        fontSize: 12.5, fontWeight: 600, cursor: disabled ? 'not-allowed' : 'pointer',
        opacity: disabled && !success ? 0.45 : 1,
      }}
    >
      {children}
    </button>
  );
}

function ConnectionDot({ bridge }: { bridge: UseBridge }) {
  const t = useT();
  const status = bridge.connection.status;
  const color = status === 'connected' ? t.ok : status === 'error' || status === 'disconnected' ? t.danger : t.warn;
  const label = status === 'connected'
    ? 'Engine ready'
    : status === 'spawning'
      ? 'Starting engine'
      : status === 'restarting'
        ? 'Restarting engine'
        : status === 'connecting'
          ? 'Connecting'
          : status === 'error'
            ? 'Engine error'
            : status === 'disconnected'
              ? 'Disconnected'
              : 'Engine idle';
  return (
    <span role="status" aria-live="polite" title={label} style={{ display: 'inline-flex', alignItems: 'center', gap: 6, fontSize: 11.5, color: t.text3 }}>
      <span style={{ width: 7, height: 7, borderRadius: 99, background: color, boxShadow: `0 0 0 3px color-mix(in oklab, ${color} 18%, transparent)` }} />
      {label}
    </span>
  );
}

function SessionRow({ session, active, disabled, onClick }: {
  session: SessionRowDto;
  active: boolean;
  disabled: boolean;
  onClick(): void;
}) {
  const t = useT();
  const when = Number.isNaN(Date.parse(session.modified_rfc3339))
    ? ''
    : new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' }).format(new Date(session.modified_rfc3339));
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      aria-current={active ? 'page' : undefined}
      style={{
        width: '100%', display: 'grid', gridTemplateColumns: '1fr auto', gap: '3px 8px',
        padding: '9px 10px', borderRadius: 8, border: 0, textAlign: 'left',
        background: active ? t.accentBg : 'transparent', color: active ? t.text : t.text2,
        cursor: disabled ? 'not-allowed' : 'pointer', opacity: disabled ? 0.55 : 1,
      }}
    >
      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 12.5, fontWeight: active ? 600 : 500 }}>
        {session.title || 'Untitled session'}
      </span>
      <span className="mono" style={{ color: t.text4, fontSize: 10 }}>{when}</span>
      <span style={{ color: t.text4, fontSize: 10.5 }}>{session.message_count} messages</span>
    </button>
  );
}

export function BetaSidebar({ bridge, onOpenSettings }: { bridge: UseBridge; onOpenSettings(): void }) {
  const t = useT();
  const workspace = bridge.bootstrap?.workspace;
  return (
    <aside style={{ width: 250, flexShrink: 0, display: 'flex', flexDirection: 'column', background: t.sidebarBg, borderRight: `0.5px solid ${t.border}`, paddingTop: 42 }}>
      <div style={{ padding: '8px 12px 12px' }}>
        <button
          type="button"
          disabled={bridge.running}
          onClick={() => invoke(bridge.pickWorkspace)}
          style={{
            width: '100%', display: 'flex', alignItems: 'center', gap: 9, padding: '9px 10px',
            borderRadius: 9, border: `0.5px solid ${t.border}`, background: t.surface,
            color: t.text, cursor: bridge.running ? 'not-allowed' : 'pointer', textAlign: 'left',
            opacity: bridge.running ? .55 : 1,
          }}
        >
          <Icon name="folder" size={15} color={t.accent} />
          <span style={{ flex: 1, minWidth: 0 }}>
            <span style={{ display: 'block', fontSize: 12.5, fontWeight: 650, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{basename(workspace?.path)}</span>
            <span style={{ display: 'block', color: workspace?.trusted ? t.ok : t.warn, fontSize: 10.5, marginTop: 2 }}>{workspace?.trusted ? 'Trusted workspace' : workspace?.path ? 'Review trust required' : 'Choose a folder'}</span>
          </span>
          <Icon name="chevron" size={13} color={t.text4} />
        </button>
      </div>

      <div style={{ display: 'flex', alignItems: 'center', padding: '3px 14px 7px' }}>
        <span style={{ flex: 1, color: t.text3, fontSize: 10.5, fontWeight: 700, letterSpacing: '.08em', textTransform: 'uppercase' }}>Sessions</span>
        <button
          type="button"
          title="New session"
          aria-label="New session"
          disabled={!bridge.connected || bridge.running}
          onClick={() => invoke(bridge.newSession)}
          style={{ width: 24, height: 24, border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: 'pointer' }}
        >
          <Icon name="plus" size={14} />
        </button>
      </div>
      <nav aria-label="Sessions" style={{ flex: 1, minHeight: 0, overflowY: 'auto', padding: '0 7px' }}>
        {bridge.desktop.sessions.length === 0 ? (
          <div style={{ padding: '14px 10px', color: t.text4, fontSize: 11.5, lineHeight: 1.5 }}>
            {bridge.connected ? 'No saved sessions in this workspace.' : 'Sessions appear after the engine connects.'}
          </div>
        ) : bridge.desktop.sessions.map((session) => (
          <SessionRow
            key={session.uuid}
            session={session}
            active={bridge.desktop.activeSessionId === session.uuid}
            disabled={bridge.running || !bridge.connected}
            onClick={() => invoke(() => bridge.resumeSession(session.uuid))}
          />
        ))}
      </nav>

      <div style={{ padding: 10, borderTop: `0.5px solid ${t.border}`, display: 'flex', flexDirection: 'column', gap: 8 }}>
        <ConnectionDot bridge={bridge} />
        <button
          type="button"
          onClick={onOpenSettings}
          style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '7px 4px', border: 0, background: 'transparent', color: t.text2, cursor: 'pointer', fontSize: 12.5 }}
        >
          <Icon name="cog" size={15} /> Settings & diagnostics
        </button>
      </div>
    </aside>
  );
}

export function BetaTopBar({ bridge, tasksOpen, onToggleTasks, theme, onTheme }: {
  bridge: UseBridge;
  tasksOpen: boolean;
  onToggleTasks(): void;
  theme: ThemeMode;
  onTheme(value: ThemeMode): void;
}) {
  const t = useT();
  return (
    <header className="drag-region" style={{ height: 52, flexShrink: 0, display: 'flex', alignItems: 'center', gap: 12, padding: '0 14px', borderBottom: `0.5px solid ${t.border}`, background: t.windowBg }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ color: t.text, fontSize: 12.5, fontWeight: 650, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{basename(bridge.bootstrap?.workspace.path)}</div>
        <div className="mono" style={{ color: t.text4, fontSize: 9.5, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{bridge.bootstrap?.workspace.path ?? 'Select a workspace to begin'}</div>
      </div>
      {bridge.usage && (
        <span className="mono" style={{ color: t.text4, fontSize: 9.5 }} title="Input + output tokens">
          {(bridge.usage.inputTokens + bridge.usage.outputTokens).toLocaleString()} tok
        </span>
      )}
      <button className="no-drag" type="button" aria-label="Toggle theme" onClick={() => onTheme(theme === 'dark' ? 'light' : 'dark')} style={{ width: 30, height: 30, display: 'grid', placeItems: 'center', borderRadius: 7, border: `0.5px solid ${t.border}`, background: 'transparent', color: t.text3, cursor: 'pointer' }}>
        <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={14} />
      </button>
      <button className="no-drag" type="button" aria-label="Toggle background tasks" aria-pressed={tasksOpen} onClick={onToggleTasks} style={{ width: 30, height: 30, display: 'grid', placeItems: 'center', borderRadius: 7, border: `0.5px solid ${tasksOpen ? t.accentBorder : t.border}`, background: tasksOpen ? t.accentBg : 'transparent', color: tasksOpen ? t.accent : t.text3, cursor: 'pointer' }}>
        <Icon name="tasks" size={15} />
      </button>
    </header>
  );
}

type SpeechRecognitionResultLike = {
  isFinal: boolean;
  [index: number]: { transcript: string };
};

type SpeechRecognitionEventLike = Event & {
  resultIndex: number;
  results: { length: number; [index: number]: SpeechRecognitionResultLike };
};

type SpeechRecognitionLike = {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  start(): void;
  stop(): void;
  onresult: ((event: SpeechRecognitionEventLike) => void) | null;
  onerror: ((event: Event & { error?: string }) => void) | null;
  onend: (() => void) | null;
};

type SpeechRecognitionConstructor = new () => SpeechRecognitionLike;

function speechRecognitionConstructor(): SpeechRecognitionConstructor | null {
  if (typeof window === 'undefined') return null;
  const browserWindow = window as Window & {
    SpeechRecognition?: SpeechRecognitionConstructor;
    webkitSpeechRecognition?: SpeechRecognitionConstructor;
  };
  return browserWindow.SpeechRecognition ?? browserWindow.webkitSpeechRecognition ?? null;
}

function modelLabel(model?: string | null): string {
  if (!model) return 'Select model';
  const name = model.split('/').at(-1) ?? model;
  return name
    .replace(/[-_]/g, ' ')
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

export function BetaComposer({ bridge, ready }: { bridge: UseBridge; ready: boolean }) {
  const t = useT();
  const [text, setText] = useState('');
  const [modelOpen, setModelOpen] = useState(false);
  const [permissionOpen, setPermissionOpen] = useState(false);
  const [goalMode, setGoalMode] = useState(false);
  const [voiceState, setVoiceState] = useState<'idle' | 'listening' | 'unsupported' | 'denied'>('idle');
  const input = useRef<HTMLTextAreaElement>(null);
  const recognition = useRef<SpeechRecognitionLike | null>(null);
  const voiceBase = useRef('');

  useEffect(() => () => {
    recognition.current?.stop();
    recognition.current = null;
  }, []);

  const stopVoice = () => {
    recognition.current?.stop();
    recognition.current = null;
    setVoiceState('idle');
  };

  const toggleVoice = () => {
    if (voiceState === 'listening') {
      stopVoice();
      return;
    }
    const SpeechRecognition = speechRecognitionConstructor();
    if (!SpeechRecognition) {
      setVoiceState('unsupported');
      return;
    }
    const next = new SpeechRecognition();
    voiceBase.current = text.trimEnd();
    next.continuous = true;
    next.interimResults = true;
    next.lang = typeof navigator !== 'undefined' && navigator.language ? navigator.language : 'en-US';
    next.onresult = (event) => {
      let transcript = '';
      for (let index = 0; index < event.results.length; index += 1) {
        transcript += event.results[index][0]?.transcript ?? '';
      }
      const prefix = voiceBase.current;
      setText(`${prefix}${prefix && transcript ? ' ' : ''}${transcript}`);
    };
    next.onerror = (event) => {
      setVoiceState(event.error === 'not-allowed' || event.error === 'service-not-allowed' ? 'denied' : 'idle');
      recognition.current = null;
    };
    next.onend = () => {
      recognition.current = null;
      setVoiceState((current) => current === 'listening' ? 'idle' : current);
    };
    recognition.current = next;
    setVoiceState('listening');
    try {
      next.start();
    } catch {
      recognition.current = null;
      setVoiceState('idle');
    }
  };

  const submit = () => {
    const value = text.trim();
    if (!value || !ready || bridge.running) return;
    if (voiceState === 'listening') stopVoice();
    setText('');
    invoke(() => bridge.sendPrompt(value));
  };
  const keyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      submit();
    }
  };
  return (
    <div style={{ flexShrink: 0, padding: '10px 18px 18px', background: t.stageBg }}>
      <div className="beta-composer" style={{ maxWidth: 980, margin: '0 auto', borderRadius: 26, border: `1px solid ${ready ? t.borderStrong : t.border}`, background: t.surface, boxShadow: '0 12px 34px rgba(0,0,0,.10)', overflow: 'visible' }}>
        <textarea
          ref={input}
          rows={2}
          value={text}
          disabled={!ready || bridge.running}
          maxLength={200_000}
          onChange={(event) => {
            setText(event.target.value);
            event.currentTarget.style.height = 'auto';
            event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 180)}px`;
          }}
          onKeyDown={keyDown}
          placeholder={!ready ? 'Complete setup to start coding…' : bridge.running ? 'LingXi is working…' : goalMode ? 'Describe the goal you want LingXi to accomplish' : 'Do anything'}
          aria-label="Prompt"
          style={{ display: 'block', width: '100%', minHeight: 86, maxHeight: 180, resize: 'none', border: 0, outline: 0, background: 'transparent', color: t.text, lineHeight: 1.45, fontSize: 17, padding: '18px 22px 4px', fontWeight: 450 }}
        />
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, minHeight: 54, padding: '0 10px 10px 14px' }}>
          <button type="button" disabled={!ready || bridge.running} aria-label="Add context" title="Add context" style={{ ...composerIconStyle(t), width: 34, height: 34 }}><Icon name="plus" size={21} color={t.text2} stroke={1.7} /></button>
          <button type="button" disabled={!ready || bridge.running} aria-expanded={permissionOpen} aria-label="Permission controls" onClick={() => { setPermissionOpen((open) => !open); setModelOpen(false); }} style={{ ...composerPillStyle(t, permissionOpen), color: t.text2 }}><Icon name="cog" size={18} color={t.text3} stroke={1.6} /><span>Custom</span></button>
          <span aria-hidden="true" style={{ width: 1, height: 24, background: t.border, margin: '0 4px' }} />
          <button type="button" disabled={!ready || bridge.running} aria-pressed={goalMode} aria-label="Toggle goal mode" onClick={() => setGoalMode((enabled) => !enabled)} style={{ ...composerPillStyle(t, goalMode), color: goalMode ? t.accent : t.text2 }}><Icon name="goal" size={18} color={goalMode ? t.accent : t.text3} stroke={1.6} /><span>Goal</span></button>

          <div style={{ flex: 1 }} />

          <div style={{ position: 'relative' }}>
            <button type="button" disabled={!ready || bridge.running || bridge.desktop.models.length === 0} aria-expanded={modelOpen} aria-label={`Model: ${modelLabel(bridge.desktop.currentModel)}`} onClick={() => { setModelOpen((open) => !open); setPermissionOpen(false); }} style={{ ...composerPillStyle(t, modelOpen), maxWidth: 280, color: t.text }}>
              <Icon name="bolt" size={18} color={t.text} stroke={2.1} />
              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{modelLabel(bridge.desktop.currentModel)}</span>
              <Icon name="chevron" size={14} color={t.text3} />
            </button>
            {modelOpen && <div style={composerMenuStyle(t, 'right')} role="menu" aria-label="Available models">
              <div style={{ padding: '7px 10px 5px', color: t.text3, fontSize: 10.5, fontWeight: 700, letterSpacing: '.08em', textTransform: 'uppercase' }}>Model</div>
              {bridge.desktop.models.map((entry) => <button key={entry} type="button" role="menuitemradio" aria-checked={entry === bridge.desktop.currentModel} onClick={() => { invoke(() => bridge.setModel(entry)); setModelOpen(false); }} style={{ display: 'flex', alignItems: 'center', gap: 9, width: '100%', padding: '9px 10px', border: 0, borderRadius: 7, background: entry === bridge.desktop.currentModel ? t.accentBg : 'transparent', color: t.text, textAlign: 'left', cursor: 'pointer', font: 'inherit', fontSize: 12.5 }}><Icon name="bolt" size={14} color={entry === bridge.desktop.currentModel ? t.accent : t.text3} /><span style={{ flex: 1 }}>{modelLabel(entry)}</span>{entry === bridge.desktop.currentModel && <Icon name="check" size={14} color={t.accent} stroke={2.2} />}</button>)}
            </div>}
          </div>
          <button type="button" disabled={!ready || bridge.running} aria-label={voiceState === 'listening' ? 'Stop voice input' : 'Start voice input'} title={voiceState === 'unsupported' ? 'Voice input is unavailable in this environment' : voiceState === 'denied' ? 'Microphone permission was denied' : 'Voice input'} onClick={toggleVoice} style={{ ...composerIconStyle(t), color: voiceState === 'listening' ? t.accent : voiceState === 'denied' ? t.danger : t.text }}><Icon name="mic" size={20} color="currentColor" stroke={voiceState === 'listening' ? 2.1 : 1.7} /></button>
          {bridge.running ? (
            <button type="button" onClick={() => invoke(() => bridge.cancel())} aria-label="Stop current turn" title="Stop" style={{ ...composerSendStyle(t, true), background: t.danger }}><Icon name="stop" size={15} color="#fff" /></button>
          ) : (
            <button type="button" disabled={!ready || !text.trim()} onClick={submit} aria-label="Send prompt" title="Send prompt" style={composerSendStyle(t, Boolean(ready && text.trim()))}><Icon name="arrowU" size={19} color={ready && text.trim() ? '#fff' : t.text4} /></button>
          )}
        </div>
        {(permissionOpen || voiceState === 'unsupported' || voiceState === 'denied') && <div style={{ position: 'relative' }}>
          {permissionOpen && <div style={composerMenuStyle(t, 'left')} role="dialog" aria-label="Permission controls">
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 7 }}><Icon name="cog" size={15} color={t.accent} /><strong style={{ color: t.text, fontSize: 12.5 }}>Permission controls</strong></div>
            <p style={{ color: t.text3, fontSize: 11.5, lineHeight: 1.45 }}>LingXi will ask before actions that need approval. Review each request in the approval panel.</p>
            <div style={{ marginTop: 9, padding: '7px 8px', borderRadius: 7, background: bridge.pendingPermission ? t.accentBg : t.surfaceHover, color: bridge.pendingPermission ? t.accent : t.text3, fontSize: 11.5 }}>{bridge.pendingPermission ? '1 request waiting for your review' : 'No pending permission requests'}</div>
          </div>}
          {voiceState === 'unsupported' && <span role="status" style={{ position: 'absolute', right: 52, bottom: 9, padding: '5px 8px', borderRadius: 7, background: t.surfaceHover, color: t.text3, fontSize: 10.5 }}>Voice input is unavailable here</span>}
          {voiceState === 'denied' && <span role="status" style={{ position: 'absolute', right: 52, bottom: 9, padding: '5px 8px', borderRadius: 7, background: t.surfaceHover, color: t.danger, fontSize: 10.5 }}>Microphone permission denied</span>}
        </div>}
      </div>
      <div style={{ maxWidth: 980, margin: '5px auto 0', padding: '0 3px', display: 'flex', justifyContent: 'space-between', color: t.text4, fontSize: 9.5 }}>
        <span>Enter to send · Shift+Enter for a new line</span>
        <span>{goalMode ? 'Goal mode enabled' : 'Review tool permissions before allowing'}</span>
      </div>
    </div>
  );
}

function composerIconStyle(t: ReturnType<typeof useT>): CSSProperties {
  return { display: 'grid', placeItems: 'center', border: 0, borderRadius: 99, background: 'transparent', color: t.text2, cursor: 'pointer', opacity: 1 };
}

function composerPillStyle(t: ReturnType<typeof useT>, active: boolean): CSSProperties {
  return { display: 'inline-flex', alignItems: 'center', gap: 7, minHeight: 34, padding: '0 9px', border: 0, borderRadius: 9, background: active ? t.surfaceHover : 'transparent', cursor: 'pointer', font: 'inherit', fontSize: 13.5, fontWeight: 500 };
}

function composerSendStyle(t: ReturnType<typeof useT>, enabled: boolean): CSSProperties {
  return { width: 38, height: 38, borderRadius: 99, border: 0, background: enabled ? t.accent : t.surfaceActive, color: enabled ? '#fff' : t.text4, display: 'grid', placeItems: 'center', cursor: enabled ? 'pointer' : 'not-allowed', opacity: enabled ? 1 : .82 };
}

function composerMenuStyle(t: ReturnType<typeof useT>, side: 'left' | 'right'): CSSProperties {
  return { position: 'absolute', bottom: 'calc(100% + 9px)', [side]: 0, zIndex: 20, width: 286, padding: 7, borderRadius: 12, border: `0.5px solid ${t.borderStrong}`, background: t.surface, boxShadow: '0 16px 40px rgba(0,0,0,.22)', animation: 'fade-in .15s ease' };
}

export function BetaTasks({ bridge, onClose }: { bridge: UseBridge; onClose(): void }) {
  const t = useT();
  const tasks = orderedTasks(bridge.desktop);
  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => { invoke(bridge.refreshTasks); }, [bridge.refreshTasks]);
  const output = selected ? bridge.desktop.taskOutput[selected] : undefined;
  return (
    <aside style={{ width: 330, flexShrink: 0, borderLeft: `0.5px solid ${t.border}`, background: t.sidebarBg, display: 'flex', flexDirection: 'column' }}>
      <div style={{ height: 52, display: 'flex', alignItems: 'center', padding: '0 12px 0 15px', borderBottom: `0.5px solid ${t.border}` }}>
        <span style={{ flex: 1, fontSize: 12.5, fontWeight: 650, color: t.text }}>Background tasks</span>
        <Button onClick={() => invoke(bridge.refreshTasks)} title="Refresh tasks"><Icon name="git" size={13} /></Button>
        <button type="button" onClick={onClose} aria-label="Close tasks" style={{ marginLeft: 6, border: 0, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="x" size={14} /></button>
      </div>
      <div style={{ flex: 1, overflow: 'auto', padding: 9 }}>
        {tasks.length === 0 ? <div style={{ padding: 18, color: t.text4, fontSize: 11.5 }}>No background tasks reported.</div> : tasks.map((task) => {
          const color = task.status.type === 'completed'
            ? t.ok
            : task.status.type === 'failed'
              ? t.danger
              : task.status.type === 'running'
                ? t.accent
                : t.text3;
          return (
            <div key={task.task_id} style={{ marginBottom: 7, padding: 10, borderRadius: 8, border: `0.5px solid ${selected === task.task_id ? t.accentBorder : t.border}`, background: selected === task.task_id ? t.accentBg : t.surface }}>
              <button type="button" onClick={() => { setSelected(task.task_id); invoke(() => bridge.taskOutput(task.task_id)); }} style={{ width: '100%', padding: 0, border: 0, background: 'transparent', color: t.text, textAlign: 'left', cursor: 'pointer' }}>
                <span style={{ display: 'flex', alignItems: 'center', gap: 7, marginBottom: 5 }}><span style={{ width: 7, height: 7, borderRadius: 99, background: color }} /><strong style={{ fontSize: 11.5 }}>{task.task_type}</strong><span style={{ marginLeft: 'auto', color, fontSize: 10 }}>{task.status.type}</span></span>
                <span style={{ display: 'block', color: t.text3, fontSize: 11.5, lineHeight: 1.45 }}>{task.description}</span>
              </button>
              {task.status.type === 'running' && <button type="button" onClick={() => invoke(() => bridge.stopTask(task.task_id))} style={{ marginTop: 7, padding: 0, border: 0, background: 'transparent', color: t.danger, fontSize: 10.5, cursor: 'pointer' }}>Stop task</button>}
            </div>
          );
        })}
        {output && <pre className="mono" style={{ marginTop: 10, padding: 10, borderRadius: 8, border: `0.5px solid ${t.border}`, background: t.windowBg, color: t.text2, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 10.5, lineHeight: 1.55 }}>{output.content || '(No output yet)'}{output.truncated ? `\n\n…output truncated (${output.totalLines} total lines)` : ''}</pre>}
      </div>
    </aside>
  );
}

export function SetupCard({ bridge }: { bridge: UseBridge }) {
  const t = useT();
  const snapshot = bridge.bootstrap;
  const workspace = snapshot?.workspace;
  const credential = snapshot?.credential;
  const providerCredentials = snapshot?.providerCredentials ?? [];
  const configuredProviders = PROVIDERS.filter((provider) => providerCredentials.find((entry) => entry.providerId === provider.id)?.configured);
  const [selectedProviderId, setSelectedProviderId] = useState(configuredProviders[0]?.id ?? 'anthropic');
  const [key, setKey] = useState('');
  const selectedProvider = providerById(selectedProviderId) ?? PROVIDERS[0];
  const selectedMetadata = providerCredentials.find((entry) => entry.providerId === selectedProvider.id)
    ?? (selectedProvider.id === 'anthropic' ? { ...credential, providerId: 'anthropic' } : undefined);
  const save = () => {
    const value = key.trim();
    if (!value) return;
    setKey('');
    invoke(() => bridge.setProviderCredential(selectedProvider.id, value));
  };
  const workspaceUnavailable = Boolean(workspace?.recovery);
  const hasProvider = configuredProviders.length > 0 || Boolean(credential?.configured);
  const step = !workspace?.path || workspaceUnavailable ? 1 : !workspace.trusted ? 2 : !hasProvider ? 3 : 4;
  return (
    <div style={{ flex: 1, display: 'grid', placeItems: 'center', padding: 30, background: t.stageBg, overflow: 'auto' }}>
      <section style={{ width: 'min(560px, 100%)', padding: 26, borderRadius: 16, background: t.surface, border: `0.5px solid ${t.border}`, boxShadow: '0 18px 50px rgba(0,0,0,.16)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 20 }}>
          <div style={{ width: 38, height: 38, borderRadius: 11, display: 'grid', placeItems: 'center', color: '#fff', background: `linear-gradient(145deg, ${t.accent}, ${t.accent2})` }}><Icon name="spark" size={20} color="#fff" /></div>
          <div><h1 style={{ color: t.text, fontSize: 17, lineHeight: 1.3 }}>Set up LingXi Code Beta</h1><p style={{ color: t.text3, fontSize: 11.5, marginTop: 3 }}>Local-first desktop coding agent for internal testing</p></div>
        </div>
        <SetupStep number={1} title="Choose a workspace" active={step === 1} complete={Boolean(workspace?.path) && !workspaceUnavailable}>
          <p>LingXi only reads and changes the folder you select.</p>
          {workspace?.recovery && <p role="alert" style={{ color: t.danger }}>{workspace.recovery.message} Choose an available folder to recover.</p>}
          <Button primary={!workspace?.path} onClick={() => invoke(bridge.pickWorkspace)}><Icon name="folder" size={14} /> {workspace?.path ? 'Change folder' : 'Choose folder'}</Button>
          {workspace?.path && <code className="mono" style={{ color: t.text3, fontSize: 10.5, overflowWrap: 'anywhere' }}>{workspace.path}</code>}
        </SetupStep>
        <SetupStep number={2} title="Trust executable workspace settings" active={step === 2} complete={Boolean(workspace?.trusted)}>
          <p>Trust enables project hooks, MCP servers and local agent settings. Review the repository first. If these files change, trust is revoked automatically.</p>
          <Button primary={step === 2} disabled={!workspace?.path || workspaceUnavailable} onClick={() => invoke(() => bridge.setWorkspaceTrusted(true))}>Trust this workspace</Button>
        </SetupStep>
          <SetupStep number={3} title="Connect a provider" active={step === 3} complete={hasProvider}>
            <p>Choose the provider and sign-in method for this desktop. Each credential is encrypted by macOS and sent to the local engine only when it starts.</p>
            {selectedMetadata?.configured && <p style={{ color: t.ok }}>Saved in macOS Keychain. Enter a new value only to replace this provider key.</p>}
            {selectedMetadata?.encryptionAvailable === false && <p style={{ color: t.danger }}>Secure credential storage is unavailable. LingXi will not store a plaintext fallback.</p>}
          <div role="radiogroup" aria-label="LLM providers" style={{ width: '100%', display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(155px, 1fr))', gap: 7 }}>
            {PROVIDERS.map((provider) => {
              const metadata = providerCredentials.find((entry) => entry.providerId === provider.id);
              const selected = provider.id === selectedProvider.id;
              return (
                <button key={provider.id} type="button" role="radio" aria-checked={selected} onClick={() => { setSelectedProviderId(provider.id); setKey(''); }} style={{ minHeight: 58, padding: '8px 9px', borderRadius: 9, border: `0.5px solid ${selected ? t.accentBorder : t.border}`, background: selected ? t.accentBg : t.windowBg, color: t.text, textAlign: 'left', cursor: 'pointer', opacity: provider.available ? 1 : .58 }}>
                  <span style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 11.5, fontWeight: 650 }}>{metadata?.configured && <Icon name="check" size={12} color={t.ok} stroke={2.5} />}{provider.label}</span>
                  <span style={{ display: 'block', color: t.text4, fontSize: 10, marginTop: 3 }}>{provider.description}</span>
                </button>
              );
            })}
          </div>
          {selectedProvider.available ? <>
            <label htmlFor="provider-credential" style={{ color: t.text2, fontSize: 11 }}>{selectedProvider.keyLabel}</label>
            <div style={{ display: 'flex', gap: 7, width: '100%' }}>
              <input id="provider-credential" type="password" autoComplete="off" spellCheck={false} value={key} disabled={selectedMetadata?.encryptionAvailable === false} onChange={(event) => setKey(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') save(); }} placeholder={selectedProvider.keyPlaceholder} aria-label={selectedProvider.keyLabel} style={{ flex: 1, minWidth: 0, height: 33, borderRadius: 8, border: `0.5px solid ${t.border}`, background: t.windowBg, color: t.text, padding: '0 9px', outline: 0 }} />
              <Button primary disabled={!key.trim() || selectedMetadata?.encryptionAvailable === false} onClick={save}>{selectedMetadata?.configured ? 'Replace' : 'Connect'}</Button>
            </div>
          </> : <p style={{ color: t.warn }}>{selectedProvider.label} uses {selectedProvider.authMethod === 'oauth' ? 'OAuth' : 'device sign-in'}, which is available from the CLI/TUI connect flow but is not wired into this desktop build yet.</p>}
        </SetupStep>
        <SetupStep number={4} title="Start the local engine" active={step === 4} complete={bridge.connected}>
          <EngineStartControl bridge={bridge} workspaceReady={Boolean(workspace?.path)} />
        </SetupStep>
      </section>
    </div>
  );
}

function EngineStartControl({ bridge, workspaceReady }: { bridge: UseBridge; workspaceReady: boolean }) {
  const t = useT();
  const status = engineLaunchStatus(bridge.connection);
  const stages = [
    { label: 'Prepare', threshold: 1 },
    { label: 'Launch', threshold: 32 },
    { label: 'Connect', threshold: 72 },
  ];
  const buttonLabel = status.phase === 'ready'
    ? 'Engine ready'
    : status.active
      ? status.label.replace(' engine', '…')
      : status.phase === 'error'
        ? 'Retry start'
        : 'Start engine';
  return (
    <div style={{ width: '100%', display: 'grid', gap: 10 }}>
      <div role="status" aria-live="polite" style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        {status.active && <span className="beta-spinner" aria-hidden="true" />}
        <strong style={{ color: status.phase === 'error' ? t.danger : status.phase === 'ready' ? t.ok : t.text2, fontSize: 12 }}>{status.label}</strong>
        <span className="mono" style={{ marginLeft: 'auto', color: t.text4, fontSize: 10 }}>{status.percent}%</span>
      </div>
      <p style={{ color: status.phase === 'error' ? t.danger : t.text3 }}>{status.detail}</p>
      <div aria-label={`Engine startup progress: ${status.percent}%`} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={status.percent} style={{ height: 6, overflow: 'hidden', borderRadius: 99, background: t.windowBg, border: `0.5px solid ${t.border}` }}>
        <span style={{ display: 'block', height: '100%', width: `${status.percent}%`, borderRadius: 99, background: status.phase === 'error' ? t.danger : status.phase === 'ready' ? t.ok : t.accent, transition: 'width 360ms ease' }} />
      </div>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: 8 }}>
        {stages.map((stage) => {
          const complete = status.phase === 'ready' || (status.percent > stage.threshold && status.phase !== 'error');
          const current = status.active && status.percent === stage.threshold;
          return <div key={stage.label} style={{ display: 'flex', alignItems: 'center', gap: 5, color: complete ? t.ok : current ? t.accent : t.text4, fontSize: 10.5 }}><span style={{ width: 6, height: 6, borderRadius: 99, background: complete ? t.ok : current ? t.accent : t.border }} />{stage.label}</div>;
        })}
      </div>
      <div style={{ display: 'flex', alignItems: 'center', gap: 9, flexWrap: 'wrap' }}>
        <Button
          primary={status.phase !== 'ready' && status.phase !== 'error'}
          success={status.phase === 'ready'}
          disabled={!workspaceReady || !status.canStart}
          onClick={() => invoke(bridge.restartBridge)}
        >
          {status.active && <span className="beta-spinner" aria-hidden="true" />}
          {buttonLabel}
        </Button>
        <ConnectionDot bridge={bridge} />
      </div>
    </div>
  );
}

function SetupStep({ number, title, active, complete, children }: { number: number; title: string; active: boolean; complete: boolean; children: ReactNode }) {
  const t = useT();
  return (
    <div style={{ display: 'grid', gridTemplateColumns: '26px 1fr', gap: 10, padding: '12px 0', borderTop: `0.5px solid ${t.border}`, opacity: active || complete ? 1 : .55 }}>
      <span style={{ width: 24, height: 24, borderRadius: 99, display: 'grid', placeItems: 'center', background: complete ? t.ok : active ? t.accent : t.windowBg, color: complete || active ? '#fff' : t.text3, fontSize: 11, fontWeight: 700 }}>{complete ? <Icon name="check" size={13} color="#fff" stroke={2.5} /> : number}</span>
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-start', gap: 8 }}>
        <h2 style={{ color: t.text, fontSize: 13.5 }}>{title}</h2>
        <div style={{ color: t.text3, fontSize: 11.5, lineHeight: 1.5, width: '100%', display: 'flex', flexDirection: 'column', alignItems: 'flex-start', gap: 8 }}>{children}</div>
      </div>
    </div>
  );
}

export function BetaSettings({ bridge, theme, onTheme, onClose }: { bridge: UseBridge; theme: ThemeMode; onTheme(value: ThemeMode): void; onClose(): void }) {
  const t = useT();
  const snapshot = bridge.bootstrap;
  const [key, setKey] = useState('');
  const [selectedProviderId, setSelectedProviderId] = useState('anthropic');
  const selectedProvider = providerById(selectedProviderId) ?? PROVIDERS[0];
  const selectedMetadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === selectedProvider.id)
    ?? (selectedProvider.id === 'anthropic' && snapshot?.credential ? { ...snapshot.credential, providerId: 'anthropic' } : undefined);
  const panelRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => { invoke(bridge.refreshDiagnostics); }, [bridge.refreshDiagnostics]);
  useEffect(() => {
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeRef.current?.focus();
    const keyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = [...(panelRef.current?.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled])') ?? [])];
      const first = focusable[0];
      const last = focusable.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    };
    document.addEventListener('keydown', keyDown);
    return () => { document.removeEventListener('keydown', keyDown); previouslyFocused?.focus(); };
  }, []);
  const save = () => { const value = key.trim(); if (!value) return; setKey(''); invoke(() => bridge.setProviderCredential(selectedProvider.id, value)); };
  return (
    <div role="dialog" aria-modal="true" aria-labelledby="lingxi-settings-title" style={{ position: 'absolute', inset: 0, zIndex: 50, display: 'grid', placeItems: 'center', background: 'rgba(0,0,0,.42)', padding: 24 }}>
      <section ref={panelRef} style={{ width: 'min(720px, 100%)', maxHeight: 'min(720px, 92vh)', overflow: 'auto', borderRadius: 15, border: `0.5px solid ${t.border}`, background: t.windowBg, boxShadow: '0 24px 70px rgba(0,0,0,.36)' }}>
        <header style={{ position: 'sticky', top: 0, zIndex: 1, display: 'flex', alignItems: 'center', padding: '14px 17px', borderBottom: `0.5px solid ${t.border}`, background: t.windowBg }}><strong id="lingxi-settings-title" style={{ flex: 1, color: t.text, fontSize: 14 }}>Settings & diagnostics</strong><button ref={closeRef} type="button" onClick={onClose} aria-label="Close settings" style={{ border: 0, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="x" size={16} /></button></header>
        <div style={{ padding: 18, display: 'grid', gap: 18 }}>
          <SettingsSection title="Appearance"><div style={{ display: 'flex', gap: 8 }}><Button primary={theme === 'dark'} onClick={() => onTheme('dark')}><Icon name="moon" size={13} /> Dark</Button><Button primary={theme === 'light'} onClick={() => onTheme('light')}><Icon name="sun" size={13} /> Light</Button></div></SettingsSection>
          <SettingsSection title="Workspace"><code className="mono" style={{ color: t.text2, fontSize: 10.5, overflowWrap: 'anywhere' }}>{snapshot?.workspace.path ?? 'Not selected'}</code><div style={{ display: 'flex', flexWrap: 'wrap', gap: 7 }}><Button disabled={bridge.running} onClick={() => invoke(bridge.pickWorkspace)}>Change folder</Button>{snapshot?.workspace.path && <Button disabled={bridge.running} danger={snapshot.workspace.trusted} onClick={() => invoke(() => bridge.setWorkspaceTrusted(!snapshot.workspace.trusted))}>{snapshot.workspace.trusted ? 'Revoke trust' : 'Trust workspace'}</Button>}</div>{snapshot?.settings.recentWorkspaces.length ? <div><p style={{ marginBottom: 6 }}>Recent workspaces</p>{snapshot.settings.recentWorkspaces.map((path) => <button key={path} type="button" disabled={bridge.running} onClick={() => invoke(() => bridge.selectRecentWorkspace(path))} className="mono" style={{ display: 'block', width: '100%', padding: '5px 0', border: 0, background: 'transparent', color: t.accent, textAlign: 'left', cursor: bridge.running ? 'not-allowed' : 'pointer', opacity: bridge.running ? .5 : 1, fontSize: 10.5, overflow: 'hidden', textOverflow: 'ellipsis' }}>{path}</button>)}</div> : null}</SettingsSection>
          <SettingsSection title="Providers">
            <p>Credentials are stored independently per provider in macOS secure storage.</p>
            <div style={{ width: '100%', display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(145px, 1fr))', gap: 7 }}>
              {PROVIDERS.map((provider) => {
                const metadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === provider.id);
                const selected = provider.id === selectedProvider.id;
                return <button key={provider.id} type="button" onClick={() => { setSelectedProviderId(provider.id); setKey(''); }} style={{ padding: '8px 9px', borderRadius: 8, border: `0.5px solid ${selected ? t.accentBorder : t.border}`, background: selected ? t.accentBg : t.surface, color: t.text, textAlign: 'left', cursor: 'pointer', opacity: provider.available ? 1 : .55 }}><span style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 11.5, fontWeight: 650 }}>{metadata?.configured && <Icon name="check" size={12} color={t.ok} stroke={2.5} />}{provider.label}</span><span style={{ display: 'block', color: t.text4, fontSize: 10, marginTop: 2 }}>{provider.available ? provider.description : 'CLI/TUI sign-in'}</span></button>;
              })}
            </div>
            {selectedProvider.available ? <>
              <label htmlFor="settings-provider-credential" style={{ color: t.text2, fontSize: 11 }}>{selectedProvider.keyLabel}</label>
              <div style={{ display: 'flex', gap: 7, width: '100%' }}><input id="settings-provider-credential" type="password" autoComplete="off" disabled={bridge.running || selectedMetadata?.encryptionAvailable === false} value={key} onChange={(event) => setKey(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') save(); }} placeholder={selectedMetadata?.configured ? 'Enter a replacement key' : selectedProvider.keyPlaceholder} aria-label={`${selectedProvider.keyLabel} for settings`} style={{ flex: 1, height: 33, borderRadius: 8, border: `0.5px solid ${t.border}`, background: t.surface, color: t.text, padding: '0 9px' }} /><Button disabled={!key.trim() || bridge.running || selectedMetadata?.encryptionAvailable === false} onClick={save}>{selectedMetadata?.configured ? 'Replace' : 'Connect'}</Button>{selectedMetadata?.configured && <Button disabled={bridge.running} danger onClick={() => invoke(() => bridge.clearProviderCredential(selectedProvider.id))}>Delete</Button>}</div>
            </> : <p style={{ color: t.warn }}>{selectedProvider.label} sign-in is currently available from the CLI/TUI connect flow.</p>}
          </SettingsSection>
          <SettingsSection title="Engine">
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><ConnectionDot bridge={bridge} /><Button onClick={() => invoke(bridge.restartBridge)}>Restart engine</Button><Button onClick={() => invoke(bridge.refresh)}>Refresh all</Button></div>
            {bridge.desktop.status && <div className="mono" style={{ color: t.text3, fontSize: 10.5 }}>session {bridge.desktop.status.session_id} · {bridge.desktop.status.model} · {bridge.desktop.status.n_messages} messages · ${bridge.desktop.status.total_cost_usd.toFixed(4)}</div>}
            {bridge.desktop.doctor && <div style={{ color: bridge.desktop.doctor.summary.failed > 0 ? t.danger : bridge.desktop.doctor.summary.warnings > 0 ? t.warn : t.ok, fontSize: 10.5 }}>Doctor: {bridge.desktop.doctor.summary.passed} passed, {bridge.desktop.doctor.summary.warnings} warnings, {bridge.desktop.doctor.summary.failed} failed</div>}
          </SettingsSection>
          <SettingsSection title="Diagnostics"><p>Sanitized lifecycle messages only. Prompts, tool payloads and credential values are excluded.</p><div style={{ display: 'flex', gap: 7 }}><Button onClick={() => invoke(bridge.copyDiagnostics)}>Copy report</Button><Button onClick={() => invoke(bridge.exportDiagnostics)}>Export JSON…</Button><Button onClick={() => invoke(bridge.refreshDiagnostics)}>Refresh</Button></div><div className="mono" style={{ maxHeight: 170, overflow: 'auto', padding: 9, borderRadius: 8, background: t.surface, border: `0.5px solid ${t.border}`, color: t.text3, fontSize: 9.5, lineHeight: 1.55 }}>{snapshot?.diagnostics.length ? snapshot.diagnostics.map((entry, index) => <div key={`${entry.timestamp}-${index}`}><span style={{ color: entry.level === 'error' ? t.danger : entry.level === 'warn' ? t.warn : t.text4 }}>{entry.timestamp} [{entry.source}/{entry.level}]</span> {entry.message}</div>) : 'No diagnostic entries.'}</div></SettingsSection>
          <SettingsSection title="About"><p>LingXi Code Desktop · Internal Beta</p><p>Local engine, explicit workspace trust, encrypted credentials, manual signed updates.</p></SettingsSection>
        </div>
      </section>
    </div>
  );
}

function SettingsSection({ title, children }: { title: string; children: ReactNode }) {
  const t = useT();
  return <section style={{ display: 'grid', gridTemplateColumns: '145px 1fr', gap: 18, paddingBottom: 18, borderBottom: `0.5px solid ${t.border}` }}><h2 style={{ color: t.text, fontSize: 12.5 }}>{title}</h2><div style={{ minWidth: 0, display: 'flex', flexDirection: 'column', alignItems: 'flex-start', gap: 9, color: t.text3, fontSize: 11.5, lineHeight: 1.5 }}>{children}</div></section>;
}

export function ErrorBanner({ bridge }: { bridge: UseBridge }) {
  const t = useT();
  if (!bridge.error) return null;
  const error = classifyDesktopError(bridge.error);
  return (
    <div role="alert" style={{ display: 'flex', alignItems: 'center', gap: 9, padding: '8px 12px', background: `color-mix(in oklab, ${t.danger} 12%, ${t.windowBg})`, borderBottom: `0.5px solid color-mix(in oklab, ${t.danger} 35%, transparent)`, color: t.danger, fontSize: 11.5 }}>
      <Icon name="circle" size={13} color={t.danger} />
      <span title={`${error.title}. ${error.detail}`} style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}><strong>{error.title}.</strong> {error.detail}</span>
      <Button onClick={bridge.clearError}>Dismiss</Button>
    </div>
  );
}
