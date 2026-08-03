import { useEffect, useMemo, useRef, useState, type CSSProperties, type ClipboardEvent, type KeyboardEvent, type ReactNode } from 'react';
import type { SessionRowDto } from '@lingxi/bridge-client';

import type { UseBridge } from '../bridge/useBridge';
import { orderedTasks } from '../bridge/desktopState';
import { engineLaunchStatus } from '../bridge/engineStatus';
import { classifyDesktopError } from '../bridge/errors';
import { useT } from '../theme/ThemeContext';
import type { ThemeMode } from '../theme/tokens';
import {
  activeFileMention,
  promptWithFileMentions,
} from '../bridge/fileMentions';
import {
  activeSlashCommand,
  filterSlashCommands,
  moveSlashSelectionIndex,
  reconcileSlashSelectionIndex,
  slashCommandText,
  slashNavigationDirection,
} from '../bridge/slashCommands';
import { groupModelReferences, modelReference } from '../bridge/modelCatalog';
import { persistProviderCredentialInput } from '../bridge/providerCredentials';
import { Icon } from './Icon';
import { PROVIDERS, providerById } from '../../shared/providers';
import { PERM_MODES } from '../data';

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
  return modelReference(model).label;
}

type FilePickerState = {
  source: 'mention' | 'button';
  query: string;
};

type RichPromptSnapshot = {
  text: string;
  files: string[];
};

const FILE_MENTION_SELECTOR = '[data-file-mention]';
const ZERO_WIDTH_SPACE = '\u200b';

function richPromptText(node: Node): string {
  if (node.nodeType === Node.TEXT_NODE) return (node.nodeValue ?? '').split(ZERO_WIDTH_SPACE).join('');
  if (node instanceof HTMLElement && node.matches(FILE_MENTION_SELECTOR)) return '';
  if (node instanceof HTMLBRElement) return '\n';

  let value = '';
  for (const child of node.childNodes) {
    if (child instanceof HTMLElement && /^(DIV|P)$/.test(child.tagName) && value && !value.endsWith('\n')) value += '\n';
    value += richPromptText(child);
  }
  return value;
}

function richPromptSnapshot(editor: HTMLElement): RichPromptSnapshot {
  const files = [...editor.querySelectorAll<HTMLElement>(FILE_MENTION_SELECTOR)]
    .map((token) => token.dataset.fileMention)
    .filter((path): path is string => Boolean(path));
  return { text: richPromptText(editor), files: [...new Set(files)] };
}

function editorSelection(editor: HTMLElement): Range {
  const selection = window.getSelection();
  if (selection?.rangeCount) {
    const current = selection.getRangeAt(0);
    if (editor.contains(current.commonAncestorContainer)) return current.cloneRange();
  }
  const end = document.createRange();
  end.selectNodeContents(editor);
  end.collapse(false);
  return end;
}

function applyEditorSelection(range: Range): void {
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function createFileMention(path: string, color: string): HTMLElement {
  const token = document.createElement('span');
  token.className = 'beta-file-mention';
  token.dataset.fileMention = path;
  token.contentEditable = 'false';
  token.title = path;
  token.setAttribute('aria-label', `File mention: ${path}`);
  token.style.color = color;

  const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  icon.setAttribute('viewBox', '0 0 18 18');
  icon.setAttribute('width', '18');
  icon.setAttribute('height', '18');
  icon.setAttribute('fill', 'none');
  icon.setAttribute('stroke', 'currentColor');
  icon.setAttribute('stroke-width', '1.45');
  icon.setAttribute('stroke-linecap', 'round');
  icon.setAttribute('stroke-linejoin', 'round');
  icon.setAttribute('aria-hidden', 'true');
  const circle = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
  circle.setAttribute('cx', '9');
  circle.setAttribute('cy', '9');
  circle.setAttribute('r', '7');
  const file = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  file.setAttribute('d', 'M6.5 5.25h4l2.25 2.25v5.25H6.5zM10.5 5.25V7.5h2.25');
  icon.append(circle, file);

  const label = document.createElement('span');
  label.textContent = basename(path);
  token.append(icon, label);
  return token;
}

export function BetaComposer({ bridge, ready }: { bridge: UseBridge; ready: boolean }) {
  const t = useT();
  const [text, setText] = useState('');
  const [modelOpen, setModelOpen] = useState(false);
  const [permissionOpen, setPermissionOpen] = useState(false);
  const [slashQuery, setSlashQuery] = useState<string | null>(null);
  const [filePicker, setFilePicker] = useState<FilePickerState | null>(null);
  const [fileResults, setFileResults] = useState<string[]>([]);
  const [fileResultsTruncated, setFileResultsTruncated] = useState(false);
  const [fileSearchStatus, setFileSearchStatus] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [fileResultIndex, setFileResultIndex] = useState(0);
  const [selectedFiles, setSelectedFiles] = useState<string[]>([]);
  const [goalMode, setGoalMode] = useState(false);
  const [voiceState, setVoiceState] = useState<'idle' | 'listening' | 'unsupported' | 'denied'>('idle');
  const [flowMode, setFlowMode] = useState(false);
  const [slashResultIndex, setSlashResultIndex] = useState(0);
  const input = useRef<HTMLDivElement>(null);
  const fileControl = useRef<HTMLDivElement>(null);
  const fileSearchInput = useRef<HTMLInputElement>(null);
  const permissionControl = useRef<HTMLDivElement>(null);
  const permissionButton = useRef<HTMLButtonElement>(null);
  const slashControl = useRef<HTMLDivElement>(null);
  const recognition = useRef<SpeechRecognitionLike | null>(null);
  const voiceBase = useRef('');
  const fileSearchRequest = useRef(0);
  const savedEditorSelection = useRef<Range | null>(null);
  const activeMentionRange = useRef<Range | null>(null);
  const activeSlashRange = useRef<Range | null>(null);
  const activeSlashQuery = useRef<string | null>(null);
  const slashDismissed = useRef(false);

  const slashCommands = useMemo(
    () => filterSlashCommands(bridge.desktop.slashCommands, slashQuery ?? ''),
    [bridge.desktop.slashCommands, slashQuery],
  );
  const modelGroups = useMemo(
    () => groupModelReferences(bridge.desktop.models),
    [bridge.desktop.models],
  );
  const slashMenuOpen = slashQuery !== null && ready && !bridge.running;

  const fileMenuOpen = Boolean(filePicker && ready && !bridge.running);

  useEffect(() => {
    setSlashResultIndex((index) => reconcileSlashSelectionIndex(
      index,
      slashQuery,
      slashQuery ?? '',
      slashCommands.length,
    ));
  }, [slashCommands.length, slashQuery]);

  useEffect(() => {
    if (!slashMenuOpen) return;
    slashControl.current
      ?.querySelector<HTMLElement>(`[data-slash-index="${slashResultIndex}"]`)
      ?.scrollIntoView({ block: 'nearest' });
  }, [slashMenuOpen, slashResultIndex]);

  useEffect(() => {
    if (!fileMenuOpen || !filePicker) {
      fileSearchRequest.current += 1;
      setFileSearchStatus('idle');
      setFileResults([]);
      setFileResultsTruncated(false);
      setFileResultIndex(0);
      return;
    }
    const request = ++fileSearchRequest.current;
    setFileSearchStatus('loading');
    const timer = window.setTimeout(() => {
      void bridge.searchWorkspaceFiles(filePicker.query)
        .then((result) => {
          if (fileSearchRequest.current !== request) return;
          setFileResults(result.files);
          setFileResultsTruncated(result.truncated);
          setFileResultIndex(0);
          setFileSearchStatus('ready');
        })
        .catch(() => {
          if (fileSearchRequest.current !== request) return;
          setFileResults([]);
          setFileResultsTruncated(false);
          setFileResultIndex(0);
          setFileSearchStatus('error');
        });
    }, 90);
    return () => window.clearTimeout(timer);
  }, [bridge.searchWorkspaceFiles, fileMenuOpen, filePicker?.query]);

  useEffect(() => {
    if (!fileMenuOpen) return;
    const pointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && !fileControl.current?.contains(event.target)) {
        setFilePicker(null);
      }
    };
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setFilePicker(null);
      if (document.activeElement === fileSearchInput.current) input.current?.focus();
    };
    document.addEventListener('pointerdown', pointerDown);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', pointerDown);
      document.removeEventListener('keydown', escape);
    };
  }, [fileMenuOpen]);

  useEffect(() => {
    if (fileMenuOpen && filePicker?.source === 'button') fileSearchInput.current?.focus();
  }, [fileMenuOpen, filePicker?.source]);

  useEffect(() => () => {
    recognition.current?.stop();
    recognition.current = null;
  }, []);

  useEffect(() => {
    if (!permissionOpen) return;
    const pointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && !permissionControl.current?.contains(event.target)) {
        setPermissionOpen(false);
      }
    };
    const keyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setPermissionOpen(false);
      permissionButton.current?.focus();
    };
    document.addEventListener('pointerdown', pointerDown);
    document.addEventListener('keydown', keyDown);
    return () => {
      document.removeEventListener('pointerdown', pointerDown);
      document.removeEventListener('keydown', keyDown);
    };
  }, [permissionOpen]);

  useEffect(() => {
    if (!slashMenuOpen) return;
    const pointerDown = (event: PointerEvent) => {
      if (
        event.target instanceof Node
        && !slashControl.current?.contains(event.target)
        && !input.current?.contains(event.target)
      ) {
        slashDismissed.current = true;
        setSlashQuery(null);
        activeSlashRange.current = null;
        activeSlashQuery.current = null;
      }
    };
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      slashDismissed.current = true;
      setSlashQuery(null);
      activeSlashRange.current = null;
      activeSlashQuery.current = null;
      input.current?.focus();
    };
    document.addEventListener('pointerdown', pointerDown);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', pointerDown);
      document.removeEventListener('keydown', escape);
    };
  }, [slashMenuOpen]);

  const syncPromptState = () => {
    const editor = input.current;
    if (!editor) return { text: '', files: [] };
    const snapshot = richPromptSnapshot(editor);
    setText(snapshot.text);
    setSelectedFiles((current) => (
      current.length === snapshot.files.length && current.every((path, index) => path === snapshot.files[index])
        ? current
        : snapshot.files
    ));
    return snapshot;
  };

  const savePromptSelection = () => {
    const editor = input.current;
    if (!editor) return;
    savedEditorSelection.current = editorSelection(editor);
  };

  const updateActiveCompletions = () => {
    const editor = input.current;
    const selection = window.getSelection();
    if (!editor || !selection?.rangeCount || !editor.contains(selection.focusNode)) return;
    savedEditorSelection.current = selection.getRangeAt(0).cloneRange();

    const node = selection.focusNode;
    const mention = node?.nodeType === Node.TEXT_NODE
      ? activeFileMention(node.nodeValue ?? '', selection.focusOffset)
      : null;
    if (mention && node) {
      const range = document.createRange();
      range.setStart(node, mention.start);
      range.setEnd(node, mention.end);
      activeMentionRange.current = range;
      setFilePicker((current) => (
        current?.source === 'mention' && current.query === mention.query
          ? current
          : { source: 'mention', query: mention.query }
      ));
      setSlashQuery(null);
      activeSlashRange.current = null;
      activeSlashQuery.current = null;
      return;
    }
    activeMentionRange.current = null;
    setFilePicker((current) => current?.source === 'mention' ? null : current);

    const slash = node?.nodeType === Node.TEXT_NODE
      ? activeSlashCommand(node.nodeValue ?? '', selection.focusOffset)
      : null;
    if (slash && node) {
      if (slashDismissed.current) return;
      const range = document.createRange();
      range.setStart(node, slash.start);
      range.setEnd(node, slash.end);
      activeSlashRange.current = range;
      const previousQuery = activeSlashQuery.current;
      activeSlashQuery.current = slash.query;
      setSlashQuery((current) => current === slash.query ? current : slash.query);
      setSlashResultIndex((index) => reconcileSlashSelectionIndex(
        index,
        previousQuery,
        slash.query,
        slashCommands.length,
      ));
      setFilePicker(null);
      setModelOpen(false);
      setPermissionOpen(false);
      return;
    }
    slashDismissed.current = false;
    activeSlashRange.current = null;
    activeSlashQuery.current = null;
    setSlashQuery(null);
  };

  const replaceVoiceText = (nextText: string) => {
    const editor = input.current;
    if (!editor) return;
    const mentions = [...editor.querySelectorAll<HTMLElement>(FILE_MENTION_SELECTOR)];
    editor.replaceChildren();
    for (const mention of mentions) editor.append(mention, document.createTextNode(ZERO_WIDTH_SPACE));
    if (nextText) editor.append(document.createTextNode(nextText));
    const range = editorSelection(editor);
    range.selectNodeContents(editor);
    range.collapse(false);
    savedEditorSelection.current = range;
    setText(nextText);
  };

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
      replaceVoiceText(`${prefix}${prefix && transcript ? ' ' : ''}${transcript}`);
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

  const toggleStandardVoice = () => {
    if (flowMode) setFlowMode(false);
    toggleVoice();
  };

  const toggleFlowMode = () => {
    if (flowMode) {
      stopVoice();
      setFlowMode(false);
      return;
    }
    setFlowMode(true);
    if (voiceState !== 'listening') toggleVoice();
  };

  const submit = () => {
    const snapshot = input.current ? richPromptSnapshot(input.current) : { text, files: selectedFiles };
    const value = promptWithFileMentions(snapshot.text, snapshot.files);
    if (!value || !ready || bridge.running) return;
    const slashCommand = snapshot.files.length === 0 ? snapshot.text.trim() : '';
    const isSlashCommand = /^\/[^\s/]+(?:\s|$)/.test(slashCommand);
    if (voiceState === 'listening') stopVoice();
    input.current?.replaceChildren();
    setText('');
    setSelectedFiles([]);
    setFilePicker(null);
    setSlashQuery(null);
    activeSlashRange.current = null;
    activeSlashQuery.current = null;
    slashDismissed.current = false;
    if (isSlashCommand) {
      invoke(() => bridge.runSlashCommand(slashCommand));
      return;
    }
    invoke(() => bridge.sendPrompt(value));
  };

  const chooseSlashCommand = (name: string) => {
    const editor = input.current;
    if (!editor) {
      setSlashQuery(null);
      return;
    }
    const insertion = activeSlashRange.current?.cloneRange() ?? editorSelection(editor);
    insertion.deleteContents();
    const command = document.createTextNode(slashCommandText(name));
    insertion.insertNode(command);
    const caret = document.createRange();
    caret.setStart(command, command.length);
    caret.collapse(true);
    applyEditorSelection(caret);
    savedEditorSelection.current = caret.cloneRange();
    activeSlashRange.current = null;
    activeSlashQuery.current = null;
    setSlashQuery(null);
    // Keep the completed token closed through the matching keyup; typing any
    // new character clears this guard in onInput.
    slashDismissed.current = true;
    setSlashResultIndex(0);
    syncPromptState();
    focusPrompt(caret);
  };

  const focusPrompt = (range?: Range | null) => {
    window.requestAnimationFrame(() => {
      const editor = input.current;
      if (!editor) return;
      editor.focus();
      applyEditorSelection(range ?? savedEditorSelection.current ?? editorSelection(editor));
    });
  };

  const chooseFile = (path: string) => {
    const editor = input.current;
    if (!filePicker || !editor) return;
    const range = filePicker.source === 'mention'
      ? activeMentionRange.current
      : savedEditorSelection.current;
    const insertion = range?.cloneRange() ?? editorSelection(editor);
    if (filePicker.source === 'mention') insertion.deleteContents();

    const duplicate = [...editor.querySelectorAll<HTMLElement>(FILE_MENTION_SELECTOR)]
      .some((token) => token.dataset.fileMention === path);
    let caret = insertion;
    if (!duplicate) {
      const token = createFileMention(path, t.accent);
      const cursorNode = document.createTextNode(ZERO_WIDTH_SPACE);
      const fragment = document.createDocumentFragment();
      fragment.append(token, cursorNode);
      insertion.insertNode(fragment);
      caret = document.createRange();
      caret.setStart(cursorNode, 1);
      caret.collapse(true);
    } else {
      caret.collapse(true);
    }
    applyEditorSelection(caret);
    savedEditorSelection.current = caret.cloneRange();
    activeMentionRange.current = null;
    syncPromptState();
    setFilePicker(null);
    setFileResults([]);
    focusPrompt(caret);
  };

  const openFileMenu = () => {
    if (filePicker?.source === 'button') {
      setFilePicker(null);
      input.current?.focus();
      return;
    }
    setSlashQuery(null);
    activeSlashRange.current = null;
    setFilePicker({ source: 'button', query: '' });
    setModelOpen(false);
    setPermissionOpen(false);
  };

  const filePickerKeyDown = (event: KeyboardEvent<HTMLInputElement | HTMLDivElement>) => {
    if (fileMenuOpen) {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        setFileResultIndex((index) => fileResults.length ? (index + 1) % fileResults.length : 0);
        return;
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        setFileResultIndex((index) => fileResults.length ? (index - 1 + fileResults.length) % fileResults.length : 0);
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        setFilePicker(null);
        input.current?.focus();
        return;
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        event.preventDefault();
        const selected = fileResults[fileResultIndex];
        if (selected) chooseFile(selected);
        else setFilePicker(null);
        return;
      }
    }
  };
  const slashPickerKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!slashMenuOpen) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      slashDismissed.current = true;
      setSlashQuery(null);
      activeSlashRange.current = null;
      activeSlashQuery.current = null;
      return;
    }
    const direction = slashNavigationDirection(event.key);
    if (direction) {
      event.preventDefault();
      setSlashResultIndex((index) => moveSlashSelectionIndex(index, direction, slashCommands.length));
      return;
    }
    if (event.key === 'Enter' || event.key === 'Tab') {
      event.preventDefault();
      const selected = slashCommands[slashResultIndex];
      if (selected) chooseSlashCommand(selected.name);
      else {
        setSlashQuery(null);
        activeSlashRange.current = null;
      }
    }
  };
  const keyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    filePickerKeyDown(event);
    if (slashMenuOpen) {
      slashPickerKeyDown(event);
      if (event.defaultPrevented) return;
    }
    if (event.defaultPrevented) return;
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      submit();
    }
  };
  const keyUp = (event: KeyboardEvent<HTMLDivElement>) => {
    // Arrow navigation changes only the active palette row. Re-running caret
    // detection on the matching keyup can reconcile against stale query state
    // and overwrite the index selected during keydown.
    if (slashMenuOpen && slashNavigationDirection(event.key)) return;
    updateActiveCompletions();
  };

  const pastePlainText = (event: ClipboardEvent<HTMLDivElement>) => {
    event.preventDefault();
    const editor = input.current;
    if (!editor) return;
    const range = editorSelection(editor);
    range.deleteContents();
    const value = event.clipboardData.getData('text/plain');
    const node = document.createTextNode(value);
    range.insertNode(node);
    range.setStart(node, node.length);
    range.collapse(true);
    applyEditorSelection(range);
    savedEditorSelection.current = range.cloneRange();
    syncPromptState();
    updateActiveCompletions();
  };
  const permissionMode = PERM_MODES.find((mode) => mode.id === bridge.desktop.permissionMode) ?? PERM_MODES[0]!;
  const promptPlaceholder = !ready
    ? 'Complete setup to start coding…'
    : bridge.running
      ? 'LingXi is working…'
      : goalMode
        ? 'Describe the goal you want LingXi to accomplish'
        : 'Do anything';
  const hasPrompt = Boolean(text.trim() || selectedFiles.length);
  return (
    <div style={{ flexShrink: 0, padding: '10px 18px 18px', background: t.stageBg }}>
      {flowMode && (
        <div
          role="status"
          aria-label="Flow mode is listening"
          style={{
            position: 'relative',
            maxWidth: 980,
            height: 156,
            margin: '0 auto 10px',
            overflow: 'hidden',
            borderRadius: 22,
            border: `1px solid ${t.accentBorder}`,
            background: `radial-gradient(circle at 50% 48%, ${t.accentBg} 0%, ${t.surface} 72%)`,
            boxShadow: '0 14px 36px rgba(0,0,0,.10)',
          }}
        >
          <div style={{ position: 'absolute', left: 16, top: 13, display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ width: 7, height: 7, borderRadius: 99, background: t.accent, boxShadow: `0 0 12px ${t.accent}` }} />
            <span style={{ color: t.text, fontSize: 12.5, fontWeight: 650 }}>心流模式</span>
          </div>
          <button
            type="button"
            aria-label="关闭心流模式"
            title="关闭心流模式"
            onClick={toggleFlowMode}
            style={{ ...composerIconStyle(t), position: 'absolute', right: 12, top: 9, width: 32, height: 32, background: t.surfaceHover }}
          >
            <Icon name="x" size={13} color={t.text3} stroke={1.9} />
          </button>
          <svg
            aria-hidden="true"
            viewBox="0 0 180 64"
            style={{ position: 'absolute', left: '50%', top: '48%', width: 210, height: 74, transform: 'translate(-50%, -50%)', color: t.accent }}
          >
            {[12, 23, 34, 48, 34, 23, 12].map((height, index) => (
              <rect key={index} x={27 + index * 20} y={(64 - height) / 2} width="7" height={height} rx="3.5" fill="currentColor" opacity={0.5 + index * 0.06}>
                <animate attributeName="height" values={`${height};${Math.max(12, 58 - Math.abs(3 - index) * 8)};${height}`} dur={`${1.05 + index * 0.09}s`} repeatCount="indefinite" />
                <animate attributeName="y" values={`${(64 - height) / 2};${(64 - Math.max(12, 58 - Math.abs(3 - index) * 8)) / 2};${(64 - height) / 2}`} dur={`${1.05 + index * 0.09}s`} repeatCount="indefinite" />
              </rect>
            ))}
          </svg>
          <div style={{ position: 'absolute', left: 0, right: 0, bottom: 13, textAlign: 'center', color: t.text3, fontSize: 11.5 }}>
            {voiceState === 'listening' ? '正在聆听 · 可继续使用下方输入框' : '轻点波形按钮继续聆听'}
          </div>
        </div>
      )}
      <div className="beta-composer" style={{ position: 'relative', maxWidth: 980, margin: '0 auto', borderRadius: 26, border: `1px solid ${ready ? t.borderStrong : t.border}`, background: t.surface, boxShadow: '0 12px 34px rgba(0,0,0,.10)', overflow: 'visible' }}>
        <div
          ref={input}
          className="beta-rich-prompt"
          role="textbox"
          contentEditable={ready && !bridge.running}
          suppressContentEditableWarning
          spellCheck
          data-placeholder={promptPlaceholder}
          data-empty={!hasPrompt ? 'true' : 'false'}
          onInput={() => { slashDismissed.current = false; syncPromptState(); updateActiveCompletions(); }}
          onFocus={() => { slashDismissed.current = false; savePromptSelection(); updateActiveCompletions(); }}
          onBlur={savePromptSelection}
          onKeyUp={keyUp}
          onMouseUp={updateActiveCompletions}
          onKeyDown={keyDown}
          onPaste={pastePlainText}
          aria-label="Prompt"
          aria-multiline="true"
          aria-disabled={!ready || bridge.running}
          aria-autocomplete="list"
          aria-controls={slashMenuOpen ? 'slash-command-results' : fileMenuOpen && filePicker?.source === 'mention' ? 'workspace-file-results' : undefined}
          aria-expanded={slashMenuOpen || (fileMenuOpen && filePicker?.source === 'mention')}
          aria-activedescendant={slashMenuOpen && slashCommands[slashResultIndex]
            ? `slash-command-result-${slashResultIndex}`
            : fileMenuOpen && filePicker?.source === 'mention' && fileResults[fileResultIndex]
              ? `workspace-file-result-${fileResultIndex}`
              : undefined}
          style={{ display: 'block', width: '100%', minHeight: 86, maxHeight: 180, overflowY: 'auto', border: 0, outline: 0, background: 'transparent', color: t.text, lineHeight: 1.45, fontSize: 17, padding: '18px 22px 4px', fontWeight: 450, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', cursor: ready && !bridge.running ? 'text' : 'default', opacity: ready && !bridge.running ? 1 : .68 }}
        />
        {slashMenuOpen && (
          <div ref={slashControl} id="slash-command-results" role="listbox" aria-label="Slash commands" style={{ ...composerMenuStyle(t, 'left'), width: 600, maxWidth: 'min(600px, calc(100vw - 44px))', maxHeight: 300, overflowY: 'auto', padding: 7 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '4px 9px 7px', borderBottom: `0.5px solid ${t.border}`, color: t.text3, fontSize: 10.5 }}>
              <strong style={{ color: t.text2, fontWeight: 700, letterSpacing: '.08em', textTransform: 'uppercase' }}>Commands</strong>
              <span className="mono" style={{ color: t.accent }}>/{slashQuery}</span>
              <span style={{ marginLeft: 'auto', color: t.text4 }}>{slashCommands.length} match{slashCommands.length === 1 ? '' : 'es'}</span>
            </div>
            {slashCommands.length === 0 && (
              <div role="status" style={{ padding: '16px 10px', color: t.text3, fontSize: 11.5 }}>
                No matching commands. Press Esc to keep the text as a prompt.
              </div>
            )}
            {slashCommands.map((entry, index) => {
              const selected = index === slashResultIndex;
              return (
                <button
                  id={`slash-command-result-${index}`}
                  data-slash-index={index}
                  key={entry.name}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  onMouseDown={(event) => event.preventDefault()}
                  onMouseEnter={() => setSlashResultIndex(index)}
                  onClick={() => chooseSlashCommand(entry.name)}
                  style={{ width: '100%', display: 'grid', gridTemplateColumns: 'minmax(92px, auto) minmax(0, 1fr) auto', gap: 10, alignItems: 'center', padding: '8px 9px', border: 0, borderRadius: 7, background: selected ? t.accentBg : 'transparent', color: t.text, textAlign: 'left', cursor: 'pointer', font: 'inherit' }}
                >
                  <span
                    className="mono"
                    style={{ color: t.accent, fontWeight: 650, borderRadius: 6, padding: '2px 0', fontSize: 11.5 }}
                  >/{entry.name}</span>
                  <span style={{ color: t.text2, fontSize: 12.5 }}>{entry.description}</span>
                  <span style={{ color: t.text4, fontSize: 10, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{entry.source}</span>
                </button>
              );
            })}
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 26, padding: '5px 9px 2px', borderTop: `0.5px solid ${t.border}`, color: t.text4, fontSize: 9.5 }}>
              <span>↑↓ Navigate</span><span>Enter / Tab Complete</span><span>Esc Close</span>
            </div>
          </div>
        )}
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, minHeight: 54, padding: '0 10px 10px 14px' }}>
          <div ref={fileControl}>
            <button type="button" disabled={!ready || bridge.running} aria-label="Search workspace files" aria-expanded={fileMenuOpen} title="Add file context (@)" onMouseDown={savePromptSelection} onClick={openFileMenu} style={{ ...composerIconStyle(t), width: 34, height: 34 }}><Icon name="plus" size={21} color={t.text2} stroke={1.7} /></button>
            {fileMenuOpen && (
              <div role="dialog" aria-label="Search workspace files" style={{ ...composerMenuStyle(t, 'left'), width: 560, maxWidth: 'min(560px, calc(100vw - 44px))', padding: 7, overflow: 'hidden' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 7, padding: '3px 4px 7px', borderBottom: `0.5px solid ${t.border}` }}>
                  <Icon name="search" size={14} color={t.text3} />
                  <input
                    ref={fileSearchInput}
                    type="text"
                    role="searchbox"
                    value={filePicker?.query ?? ''}
                    onChange={(event) => setFilePicker((current) => current ? { ...current, query: event.target.value } : current)}
                    onKeyDown={filePickerKeyDown}
                    placeholder="Search workspace files"
                    aria-label="File search query"
                    aria-controls="workspace-file-results"
                    aria-activedescendant={fileResults[fileResultIndex] ? `workspace-file-result-${fileResultIndex}` : undefined}
                    style={{ minWidth: 0, flex: 1, height: 30, padding: '0 3px', border: 0, outline: 0, background: 'transparent', color: t.text, font: 'inherit', fontSize: 12.5 }}
                  />
                  <span className="mono" style={{ color: t.text4, fontSize: 9.5 }}>@ file</span>
                  {filePicker?.query && <button type="button" aria-label="Clear file search" title="Clear search" onClick={() => { setFilePicker((current) => current ? { ...current, query: '' } : current); fileSearchInput.current?.focus(); }} style={{ width: 26, height: 26, display: 'grid', placeItems: 'center', padding: 0, border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="x" size={11} /></button>}
                  <button type="button" aria-label="Close file search" title="Close" onClick={() => { setFilePicker(null); input.current?.focus(); }} style={{ width: 26, height: 26, display: 'grid', placeItems: 'center', padding: 0, border: 0, borderRadius: 6, background: t.surfaceHover, color: t.text2, cursor: 'pointer' }}><Icon name="x" size={13} /></button>
                </div>
                <div id="workspace-file-results" role="listbox" aria-label="Workspace files" style={{ maxHeight: 310, overflowY: 'auto', padding: '5px 0' }}>
                  {fileSearchStatus === 'loading' && <div role="status" style={{ padding: '14px 10px', color: t.text3, fontSize: 11.5 }}>Searching workspace…</div>}
                  {fileSearchStatus === 'error' && <div role="alert" style={{ padding: '14px 10px', color: t.danger, fontSize: 11.5 }}>Could not search this workspace.</div>}
                  {fileSearchStatus === 'ready' && fileResults.length === 0 && <div role="status" style={{ padding: '14px 10px', color: t.text3, fontSize: 11.5 }}>No matching files.</div>}
                  {fileResults.map((path, index) => {
                    const slash = path.lastIndexOf('/');
                    const directory = slash >= 0 ? path.slice(0, slash) : 'workspace root';
                    const selected = index === fileResultIndex;
                    return (
                      <button
                        id={`workspace-file-result-${index}`}
                        key={path}
                        type="button"
                        role="option"
                        aria-selected={selected}
                        onMouseDown={(event) => event.preventDefault()}
                        onMouseEnter={() => setFileResultIndex(index)}
                        onClick={() => chooseFile(path)}
                        style={{ width: '100%', display: 'grid', gridTemplateColumns: '24px minmax(0, 1fr)', gap: 8, alignItems: 'center', padding: '7px 9px', border: 0, borderRadius: 7, background: selected ? t.accentBg : 'transparent', color: t.text, textAlign: 'left', cursor: 'pointer', font: 'inherit' }}
                      >
                        <Icon name="file" size={15} color={selected ? t.accent : t.text3} />
                        <span style={{ minWidth: 0 }}>
                          <span style={{ display: 'block', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 12.5, fontWeight: 570 }}>{basename(path)}</span>
                          <span className="mono" style={{ display: 'block', marginTop: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: t.text4, fontSize: 9.5 }}>{directory}</span>
                        </span>
                      </button>
                    );
                  })}
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 30, padding: '5px 9px 2px', borderTop: `0.5px solid ${t.border}`, color: t.text4, fontSize: 9.5 }}>
                  <span>↑↓ Navigate</span><span>Enter / Tab Add</span><span>Esc Close</span>
                  {fileResultsTruncated && <span style={{ marginLeft: 'auto' }}>More matches available — keep typing</span>}
                </div>
              </div>
            )}
          </div>
          <div ref={permissionControl} style={{ position: 'relative' }}>
            <button
              ref={permissionButton}
              type="button"
              disabled={!ready || bridge.running}
              aria-haspopup="menu"
              aria-expanded={permissionOpen}
              aria-label={`Permission mode: ${permissionMode.label}`}
              title="Change permission mode"
              onMouseDown={() => { setSlashQuery(null); activeSlashRange.current = null; }}
              onClick={() => { setPermissionOpen((open) => !open); setModelOpen(false); }}
              style={{
                ...composerPillStyle(t, permissionOpen),
                color: permissionMode.danger ? t.danger : permissionOpen ? t.accent : t.text2,
                background: permissionOpen ? t.accentBg : permissionMode.danger ? `color-mix(in oklab, ${t.danger} 9%, transparent)` : 'transparent',
              }}
            >
              <Icon name={permissionMode.icon} size={18} color="currentColor" stroke={1.65} />
              <span>{permissionMode.shortLabel}</span>
              <Icon name="chevron" size={12} color="currentColor" stroke={1.8} />
            </button>
            {permissionOpen && (
              <div
                style={{ ...composerMenuStyle(t, 'left'), width: 480, maxWidth: 'min(480px, calc(100vw - 44px))', maxHeight: 'min(470px, calc(100vh - 150px))', overflowY: 'auto', padding: 9 }}
                role="menu"
                aria-label="Permission modes"
              >
                <div style={{ padding: '4px 9px 8px', display: 'flex', alignItems: 'baseline', gap: 9 }}>
                  <strong style={{ color: t.text, fontSize: 12.5, fontWeight: 650 }}>How should LingXi actions be approved?</strong>
                  <span style={{ marginLeft: 'auto', color: t.text4, fontSize: 10.5 }}>Current session</span>
                </div>
                {PERM_MODES.map((mode) => {
                  const selected = mode.id === bridge.desktop.permissionMode;
                  const color = mode.danger ? t.danger : selected ? t.accent : t.text2;
                  return (
                    <button
                      key={mode.id}
                      type="button"
                      role="menuitemradio"
                      aria-checked={selected}
                      onClick={() => {
                        if (!selected) invoke(() => bridge.setPermissionMode(mode.id));
                        setPermissionOpen(false);
                      }}
                      style={{
                        width: '100%', display: 'grid', gridTemplateColumns: '28px minmax(0, 1fr) 18px',
                        alignItems: 'center', gap: 9, padding: '8px 9px', border: 0, borderRadius: 9,
                        background: selected ? t.accentBg : 'transparent', color, textAlign: 'left',
                        cursor: 'pointer', font: 'inherit',
                      }}
                    >
                      <span style={{ width: 28, height: 28, display: 'grid', placeItems: 'center', color }}>
                        <Icon name={mode.icon} size={19} color="currentColor" stroke={1.65} />
                      </span>
                      <span style={{ minWidth: 0 }}>
                        <span style={{ display: 'block', color, fontSize: 13, fontWeight: 570, lineHeight: 1.25 }}>{mode.label}</span>
                        <span style={{ display: 'block', marginTop: 2, color: mode.danger ? t.danger : t.text3, fontSize: 11, lineHeight: 1.35 }}>{mode.description}</span>
                      </span>
                      {selected && <Icon name="check" size={16} color={color} stroke={2.2} />}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
          <span aria-hidden="true" style={{ width: 1, height: 24, background: t.border, margin: '0 4px' }} />
          <button type="button" disabled={!ready || bridge.running} aria-pressed={goalMode} aria-label="Toggle goal mode" onClick={() => setGoalMode((enabled) => !enabled)} style={{ ...composerPillStyle(t, goalMode), color: goalMode ? t.accent : t.text2 }}><Icon name="goal" size={18} color={goalMode ? t.accent : t.text3} stroke={1.6} /><span>Goal</span></button>

          <div style={{ flex: 1 }} />

          <div style={{ position: 'relative' }}>
            <button
              type="button"
              disabled={!ready || bridge.running || bridge.desktop.models.length === 0}
              aria-expanded={modelOpen}
              aria-label={`Model: ${modelLabel(bridge.desktop.currentModel)}`}
              onMouseDown={() => { setSlashQuery(null); activeSlashRange.current = null; }}
              onClick={() => { setModelOpen((open) => !open); setPermissionOpen(false); }}
              style={{ ...composerPillStyle(t, modelOpen), maxWidth: 280, color: t.text }}
            >
              <Icon name="bolt" size={18} color={t.text} stroke={2.1} />
              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{modelLabel(bridge.desktop.currentModel)}</span>
              <Icon name="chevron" size={14} color={t.text3} />
            </button>
            {modelOpen && <div style={composerMenuStyle(t, 'right')} role="menu" aria-label="Available models">
              {modelGroups.map((group, groupIndex) => (
                <section
                  key={group.providerId ?? 'unqualified'}
                  aria-labelledby={`desktop-model-provider-${group.providerId ?? 'other'}`}
                  style={groupIndex === 0 ? undefined : { marginTop: 5, paddingTop: 5, borderTop: `0.5px solid ${t.border}` }}
                >
                  <div id={`desktop-model-provider-${group.providerId ?? 'other'}`} style={{ padding: '7px 10px 5px', color: t.text3, fontSize: 10.5, fontWeight: 700, letterSpacing: '.08em', textTransform: 'uppercase' }}>{group.providerLabel}</div>
                  {group.models.map((entry) => (
                    <button
                      key={entry.reference}
                      type="button"
                      role="menuitemradio"
                      aria-checked={entry.reference === bridge.desktop.currentModel}
                      onClick={() => {
                        invoke(() => bridge.setModel(entry.reference));
                        setModelOpen(false);
                      }}
                      style={{
                        display: 'flex', alignItems: 'center', gap: 9, width: '100%',
                        padding: '9px 10px', border: 0, borderRadius: 7,
                        background: entry.reference === bridge.desktop.currentModel ? t.accentBg : 'transparent',
                        color: t.text, textAlign: 'left', cursor: 'pointer', font: 'inherit', fontSize: 12.5,
                      }}
                    >
                      <Icon name="bolt" size={14} color={entry.reference === bridge.desktop.currentModel ? t.accent : t.text3} />
                      <span style={{ flex: 1 }}>{entry.label}</span>
                      {entry.reference === bridge.desktop.currentModel && <Icon name="check" size={14} color={t.accent} stroke={2.2} />}
                    </button>
                  ))}
                </section>
              ))}
            </div>}
          </div>
          <button type="button" disabled={!ready || bridge.running} aria-label={voiceState === 'listening' && !flowMode ? 'Stop ordinary recording' : 'Start ordinary recording'} title={voiceState === 'unsupported' ? 'Voice input is unavailable in this environment' : voiceState === 'denied' ? 'Microphone permission was denied' : '普通录音'} onClick={toggleStandardVoice} style={{ ...composerIconStyle(t), width: 36, height: 36, color: voiceState === 'listening' && !flowMode ? t.accent : voiceState === 'denied' ? t.danger : t.text }}><Icon name="mic" size={20} color="currentColor" stroke={voiceState === 'listening' && !flowMode ? 2.1 : 1.7} /></button>
          <button
            type="button"
            disabled={!ready || bridge.running}
            aria-label={flowMode ? '关闭心流模式' : '开启心流模式'}
            aria-pressed={flowMode}
            title={flowMode ? '关闭心流模式' : '开启心流模式'}
            onClick={toggleFlowMode}
            style={{ ...composerIconStyle(t), width: 42, height: 42, background: flowMode ? t.accent : t.text, color: t.windowBg, boxShadow: flowMode ? `0 0 0 4px ${t.accentBg}` : 'none' }}
          >
            <Icon name="waveform" size={21} color={flowMode ? '#fff' : t.windowBg} stroke={2.15} />
          </button>
          {bridge.running ? (
            <button
              type="button"
              disabled={bridge.isCancelling}
              onClick={() => invoke(() => bridge.cancel())}
              aria-label={bridge.isCancelling ? 'Stopping current turn' : 'Stop current turn'}
              title={bridge.isCancelling ? 'Stopping…' : 'Stop'}
              style={{ ...composerSendStyle(t, true), background: t.danger, cursor: bridge.isCancelling ? 'wait' : 'pointer', opacity: bridge.isCancelling ? .7 : 1 }}
            ><Icon name="stop" size={15} color="#fff" /></button>
          ) : (
            <button type="button" disabled={!ready || !hasPrompt} onClick={submit} aria-label="Send prompt" title="Send prompt" style={composerSendStyle(t, Boolean(ready && hasPrompt))}><Icon name="arrowU" size={19} color={ready && hasPrompt ? '#fff' : t.text4} /></button>
          )}
        </div>
        {(voiceState === 'unsupported' || voiceState === 'denied') && <div style={{ position: 'relative' }}>
          {voiceState === 'unsupported' && <span role="status" style={{ position: 'absolute', right: 52, bottom: 9, padding: '5px 8px', borderRadius: 7, background: t.surfaceHover, color: t.text3, fontSize: 10.5 }}>Voice input is unavailable here</span>}
          {voiceState === 'denied' && <span role="status" style={{ position: 'absolute', right: 52, bottom: 9, padding: '5px 8px', borderRadius: 7, background: t.surfaceHover, color: t.danger, fontSize: 10.5 }}>Microphone permission denied</span>}
        </div>}
      </div>
      <div style={{ maxWidth: 980, margin: '5px auto 0', padding: '0 3px', display: 'flex', justifyContent: 'space-between', color: t.text4, fontSize: 9.5 }}>
        <span>Enter to send · Shift+Enter for a new line · @ files · / commands</span>
        <span>{goalMode ? 'Goal mode enabled' : permissionMode.description}</span>
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
  const providerCredentials = snapshot?.providerCredentials ?? [];
  const configuredProviders = PROVIDERS.filter((provider) => providerCredentials.find((entry) => entry.providerId === provider.id)?.configured);
  const [selectedProviderId, setSelectedProviderId] = useState(configuredProviders[0]?.id ?? 'anthropic');
  const [key, setKey] = useState('');
  const selectedProvider = providerById(selectedProviderId) ?? PROVIDERS[0];
  const selectedMetadata = providerCredentials.find((entry) => entry.providerId === selectedProvider.id);
  const save = () => {
    const submitted = key;
    invoke(async () => {
      const saved = await persistProviderCredentialInput(selectedProvider.id, submitted, bridge.setProviderCredential);
      if (saved) setKey((current) => current === submitted ? '' : current);
    });
  };
  const workspaceUnavailable = Boolean(workspace?.recovery);
  const hasProvider = configuredProviders.length > 0;
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
            <p>Choose a provider for this desktop. API keys are persisted by the local engine and shared with CLI and TUI.</p>
            {selectedMetadata?.runtimeOnly && <p style={{ color: t.ok }}>Available to the running engine for this app launch. LingXi has not stored this external credential.</p>}
            {selectedMetadata?.configured && selectedMetadata.encryptionAvailable && !selectedMetadata.runtimeOnly && <p style={{ color: t.ok }}>Saved in the shared macOS login Keychain used by Desktop, CLI, and TUI. Generic API keys do not appear in the Passwords app.</p>}
            {selectedMetadata?.configured && !selectedMetadata.encryptionAvailable && !selectedMetadata.runtimeOnly && <p style={{ color: t.warn }}>macOS Keychain is unavailable. Saved in the shared owner-only local fallback used by Desktop, CLI, and TUI.</p>}
            {!selectedMetadata?.configured && selectedMetadata?.encryptionAvailable === false && <p style={{ color: t.warn }}>macOS Keychain is unavailable. Connecting will use the shared owner-only local fallback used by Desktop, CLI, and TUI.</p>}
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
              <input id="provider-credential" type="password" autoComplete="off" spellCheck={false} value={key} onChange={(event) => setKey(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') save(); }} placeholder={selectedProvider.keyPlaceholder} aria-label={selectedProvider.keyLabel} style={{ flex: 1, minWidth: 0, height: 33, borderRadius: 8, border: `0.5px solid ${t.border}`, background: t.windowBg, color: t.text, padding: '0 9px', outline: 0 }} />
              <Button primary disabled={!key.trim()} onClick={save}>{selectedMetadata?.runtimeOnly ? 'Use entered key' : selectedMetadata?.configured ? 'Replace' : 'Connect'}</Button>
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
  const selectedMetadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === selectedProvider.id);
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
  const save = () => {
    const submitted = key;
    invoke(async () => {
      const saved = await persistProviderCredentialInput(selectedProvider.id, submitted, bridge.setProviderCredential);
      if (saved) setKey((current) => current === submitted ? '' : current);
    });
  };
  return (
    <div role="dialog" aria-modal="true" aria-labelledby="lingxi-settings-title" style={{ position: 'absolute', inset: 0, zIndex: 50, display: 'grid', placeItems: 'center', background: 'rgba(0,0,0,.42)', padding: 24 }}>
      <section ref={panelRef} style={{ width: 'min(720px, 100%)', maxHeight: 'min(720px, 92vh)', overflow: 'auto', borderRadius: 15, border: `0.5px solid ${t.border}`, background: t.windowBg, boxShadow: '0 24px 70px rgba(0,0,0,.36)' }}>
        <header style={{ position: 'sticky', top: 0, zIndex: 1, display: 'flex', alignItems: 'center', padding: '14px 17px', borderBottom: `0.5px solid ${t.border}`, background: t.windowBg }}><strong id="lingxi-settings-title" style={{ flex: 1, color: t.text, fontSize: 14 }}>Settings & diagnostics</strong><button ref={closeRef} type="button" onClick={onClose} aria-label="Close settings" style={{ border: 0, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="x" size={16} /></button></header>
        <div style={{ padding: 18, display: 'grid', gap: 18 }}>
          <SettingsSection title="Appearance"><div style={{ display: 'flex', gap: 8 }}><Button primary={theme === 'dark'} onClick={() => onTheme('dark')}><Icon name="moon" size={13} /> Dark</Button><Button primary={theme === 'light'} onClick={() => onTheme('light')}><Icon name="sun" size={13} /> Light</Button></div></SettingsSection>
          <SettingsSection title="Workspace"><code className="mono" style={{ color: t.text2, fontSize: 10.5, overflowWrap: 'anywhere' }}>{snapshot?.workspace.path ?? 'Not selected'}</code><div style={{ display: 'flex', flexWrap: 'wrap', gap: 7 }}><Button disabled={bridge.running} onClick={() => invoke(bridge.pickWorkspace)}>Change folder</Button>{snapshot?.workspace.path && <Button disabled={bridge.running} danger={snapshot.workspace.trusted} onClick={() => invoke(() => bridge.setWorkspaceTrusted(!snapshot.workspace.trusted))}>{snapshot.workspace.trusted ? 'Revoke trust' : 'Trust workspace'}</Button>}</div>{snapshot?.settings.recentWorkspaces.length ? <div><p style={{ marginBottom: 6 }}>Recent workspaces</p>{snapshot.settings.recentWorkspaces.map((path) => <button key={path} type="button" disabled={bridge.running} onClick={() => invoke(() => bridge.selectRecentWorkspace(path))} className="mono" style={{ display: 'block', width: '100%', padding: '5px 0', border: 0, background: 'transparent', color: t.accent, textAlign: 'left', cursor: bridge.running ? 'not-allowed' : 'pointer', opacity: bridge.running ? .5 : 1, fontSize: 10.5, overflow: 'hidden', textOverflow: 'ellipsis' }}>{path}</button>)}</div> : null}</SettingsSection>
          <SettingsSection title="Providers">
            <p>Desktop, CLI, and TUI share one credential store. It uses the macOS login Keychain when available and an owner-only local fallback otherwise.</p>
            <div style={{ width: '100%', display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(145px, 1fr))', gap: 7 }}>
              {PROVIDERS.map((provider) => {
                const metadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === provider.id);
                const selected = provider.id === selectedProvider.id;
                return <button key={provider.id} type="button" onClick={() => { setSelectedProviderId(provider.id); setKey(''); }} style={{ padding: '8px 9px', borderRadius: 8, border: `0.5px solid ${selected ? t.accentBorder : t.border}`, background: selected ? t.accentBg : t.surface, color: t.text, textAlign: 'left', cursor: 'pointer', opacity: provider.available ? 1 : .55 }}><span style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 11.5, fontWeight: 650 }}>{metadata?.configured && <Icon name="check" size={12} color={t.ok} stroke={2.5} />}{provider.label}</span><span style={{ display: 'block', color: t.text4, fontSize: 10, marginTop: 2 }}>{provider.available ? provider.description : 'CLI/TUI sign-in'}</span></button>;
              })}
            </div>
            {selectedMetadata?.runtimeOnly && <p style={{ color: t.ok }}>The running engine received this credential from an external runtime source. LingXi has not stored it.</p>}
            {selectedMetadata?.configured && selectedMetadata.encryptionAvailable && !selectedMetadata.runtimeOnly && <p style={{ color: t.ok }}>Persisted securely on this Mac. Generic API keys are intentionally not listed in the Passwords app.</p>}
            {selectedMetadata?.configured && !selectedMetadata.encryptionAvailable && !selectedMetadata.runtimeOnly && <p style={{ color: t.warn }}>macOS Keychain is unavailable. This credential is in the shared owner-only local fallback used by Desktop, CLI, and TUI.</p>}
            {!selectedMetadata?.configured && selectedMetadata?.encryptionAvailable === false && <p style={{ color: t.warn }}>macOS Keychain is unavailable. Connecting will use the shared owner-only local fallback used by Desktop, CLI, and TUI.</p>}
            {selectedProvider.available ? <>
              <label htmlFor="settings-provider-credential" style={{ color: t.text2, fontSize: 11 }}>{selectedProvider.keyLabel}</label>
              <div style={{ display: 'flex', gap: 7, width: '100%' }}><input id="settings-provider-credential" type="password" autoComplete="off" disabled={bridge.running} value={key} onChange={(event) => setKey(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') save(); }} placeholder={selectedMetadata?.configured ? 'Enter a replacement key' : selectedProvider.keyPlaceholder} aria-label={`${selectedProvider.keyLabel} for settings`} style={{ flex: 1, height: 33, borderRadius: 8, border: `0.5px solid ${t.border}`, background: t.surface, color: t.text, padding: '0 9px' }} /><Button disabled={!key.trim() || bridge.running} onClick={save}>{selectedMetadata?.runtimeOnly ? 'Use entered key' : selectedMetadata?.configured ? 'Replace' : 'Connect'}</Button>{selectedMetadata?.configured && !selectedMetadata.runtimeOnly && <Button disabled={bridge.running} danger onClick={() => invoke(() => bridge.clearProviderCredential(selectedProvider.id))}>Delete</Button>}</div>
            </> : <p style={{ color: t.warn }}>{selectedProvider.label} sign-in is currently available from the CLI/TUI connect flow.</p>}
          </SettingsSection>
          <SettingsSection title="Engine">
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><ConnectionDot bridge={bridge} /><Button onClick={() => invoke(bridge.restartBridge)}>Restart engine</Button><Button onClick={() => invoke(bridge.refresh)}>Refresh all</Button></div>
            {bridge.desktop.status && <div className="mono" style={{ color: t.text3, fontSize: 10.5 }}>session {bridge.desktop.status.session_id} · {bridge.desktop.status.model} · {bridge.desktop.status.n_messages} messages · ${bridge.desktop.status.total_cost_usd.toFixed(4)}</div>}
            {bridge.desktop.doctor && <div style={{ color: bridge.desktop.doctor.summary.failed > 0 ? t.danger : bridge.desktop.doctor.summary.warnings > 0 ? t.warn : t.ok, fontSize: 10.5 }}>Doctor: {bridge.desktop.doctor.summary.passed} passed, {bridge.desktop.doctor.summary.warnings} warnings, {bridge.desktop.doctor.summary.failed} failed</div>}
          </SettingsSection>
          <SettingsSection title="Diagnostics"><p>Sanitized lifecycle messages only. Prompts, tool payloads and credential values are excluded.</p><div style={{ display: 'flex', gap: 7 }}><Button onClick={() => invoke(bridge.copyDiagnostics)}>Copy report</Button><Button onClick={() => invoke(bridge.exportDiagnostics)}>Export JSON…</Button><Button onClick={() => invoke(bridge.refreshDiagnostics)}>Refresh</Button></div><div className="mono" style={{ maxHeight: 170, overflow: 'auto', padding: 9, borderRadius: 8, background: t.surface, border: `0.5px solid ${t.border}`, color: t.text3, fontSize: 9.5, lineHeight: 1.55 }}>{snapshot?.diagnostics.length ? snapshot.diagnostics.map((entry, index) => <div key={`${entry.timestamp}-${index}`}><span style={{ color: entry.level === 'error' ? t.danger : entry.level === 'warn' ? t.warn : t.text4 }}>{entry.timestamp} [{entry.source}/{entry.level}]</span> {entry.message}</div>) : 'No diagnostic entries.'}</div></SettingsSection>
          <SettingsSection title="About"><p>LingXi Code Desktop · Internal Beta</p><p>Local engine, explicit workspace trust, shared credential storage, manual signed updates.</p></SettingsSection>
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
