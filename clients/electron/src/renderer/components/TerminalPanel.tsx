import { useCallback, useEffect, useRef, useState, type CSSProperties, type PointerEvent } from 'react';
import type { Terminal as XTerminal } from '@xterm/xterm';
import type { TerminalScope, TerminalSnapshot } from '../../shared/terminal';
import { useT } from '../theme/ThemeContext';

const HEIGHT_KEY = 'lingxi.terminal.height.v1';
export const terminalScopeKey = (scope: TerminalScope) => JSON.stringify([scope.projectPath, scope.sessionId]);
export const clampTerminalHeight = (height: number, available: number) => Math.max(100, Math.min(height, Math.max(100, available - 240)));

export function useTerminalPanel(scope: TerminalScope | null, enabled: boolean) {
  const [tabs, setTabs] = useState<TerminalSnapshot[]>([]);
  const [states, setStates] = useState<Record<string, { open: boolean; selected?: string }>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [focusRequest, setFocusRequest] = useState<{ id: string; serial: number } | null>(null);
  const creating = useRef(false);
  const closedIds = useRef(new Set<string>());
  const exits = useRef(new Map<string, number | null>());
  const scopes = useRef(new Map<string, TerminalScope>());
  const current = useRef(scope); current.current = scope;
  const key = scope ? terminalScopeKey(scope) : '';
  const state = states[key];
  const visibleTabs = tabs.filter(tab => terminalScopeKey(tab.scope) === key);
  const selected = visibleTabs.find(tab => tab.id === state?.selected) ?? visibleTabs[0];
  const merge = useCallback((records: TerminalSnapshot[]) => setTabs(previous => {
    const next = new Map(previous.map(tab => [tab.id, tab]));
    for (const tab of records) {
      if (closedIds.current.has(tab.id)) continue;
      next.set(tab.id, { ...tab, scope: scopes.current.get(tab.id) ?? tab.scope, ...(exits.current.has(tab.id) ? { status: 'exited' as const, exitCode: exits.current.get(tab.id)! } : {}) });
    }
    return [...next.values()];
  }), []);
  useEffect(() => {
    if (!window.lingxi?.terminal) return;
    return window.lingxi!.terminal.onEvent(event => {
      if (event.kind === 'output' || event.kind === 'reset') return;
      if (event.kind === 'closed') { closedIds.current.add(event.terminalId); setTabs(previous => previous.filter(tab => tab.id !== event.terminalId)); }
      if (event.kind === 'exit') { exits.current.set(event.terminalId, event.exitCode); setTabs(previous => previous.map(tab => tab.id === event.terminalId ? { ...tab, status: 'exited', exitCode: event.exitCode } : tab)); }
      if (event.kind === 'scope') {
        scopes.current.set(event.terminalId, event.scope);
        setTabs(previous => previous.map(tab => tab.id === event.terminalId ? { ...tab, scope: event.scope } : tab));
        setStates(previous => {
          const owner = Object.entries(previous).find(([, value]) => value.selected === event.terminalId);
          return owner ? { ...previous, [terminalScopeKey(event.scope)]: owner[1] } : previous;
        });
      }
    });
  }, []);
  useEffect(() => {
    setFocusRequest(null);
    setError(null);
    if (!scope || !window.lingxi?.terminal) return;
    let cancelled = false;
    void window.lingxi!.terminal.list(scope).then(records => { if (!cancelled) merge(records); }).catch(reason => { if (!cancelled) setError(String(reason)); });
    return () => { cancelled = true; };
  }, [key, merge]);
  const create = async () => {
    const owner = current.current;
    if (!owner || creating.current) return;
    creating.current = true;
    setBusy(true); setError(null);
    try {
      const tab = await window.lingxi!.terminal.create(owner);
      merge([tab]);
      const ownerKey = terminalScopeKey(tab.scope);
      setStates(previous => ({ ...previous, [ownerKey]: { open: true, selected: tab.id } }));
      if (current.current && terminalScopeKey(current.current) === ownerKey) setFocusRequest({ id: tab.id, serial: Date.now() });
    } catch (reason) { setError(String(reason)); }
    finally { creating.current = false; setBusy(false); }
  };
  const toggle = () => {
    if (!scope || !enabled) return;
    const open = !state?.open;
    setStates(previous => ({ ...previous, [key]: { ...previous[key], open } }));
    if (open && !visibleTabs.length) void create();
    else if (open && selected) setFocusRequest({ id: selected.id, serial: Date.now() });
  };
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (enabled && event.ctrlKey && event.code === 'Backquote' && !event.altKey && !event.metaKey) { event.preventDefault(); toggle(); }
    };
    window.addEventListener('keydown', keydown);
    return () => window.removeEventListener('keydown', keydown);
  });
  return { tabs, visibleTabs, selected, open: enabled && Boolean(state?.open), error, busy, focusRequest, toggle, create,
    hide: () => setStates(previous => ({ ...previous, [key]: { ...previous[key], open: false } })),
    select: (id: string) => { setStates(previous => ({ ...previous, [key]: { open: true, selected: id } })); setFocusRequest({ id, serial: Date.now() }); },
    close: async (id: string) => { try { await window.lingxi!.terminal.close(id); closedIds.current.add(id); setTabs(previous => previous.filter(tab => tab.id !== id)); return true; } catch (reason) { setError(String(reason)); return false; } },
  };
}

export function TerminalPanel({ controller }: { controller: ReturnType<typeof useTerminalPanel> }) {
  const t = useT();
  const ref = useRef<HTMLElement>(null);
  const [height, setHeight] = useState(() => { try { return Number(localStorage.getItem(HEIGHT_KEY)) || 240; } catch { return 240; } });
  const [available, setAvailable] = useState(window.innerHeight);
  useEffect(() => {
    const parent = ref.current?.parentElement;
    if (!parent) return;
    const observer = new ResizeObserver(() => setAvailable(parent.clientHeight));
    observer.observe(parent); return () => observer.disconnect();
  }, []);
  const saveHeight = (value: number) => { setHeight(value); try { localStorage.setItem(HEIGHT_KEY, String(value)); } catch { /* Storage can be disabled. */ } };
  const resize = (event: PointerEvent<HTMLDivElement>) => {
    event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId);
    const start = event.clientY; const initial = clampTerminalHeight(height, available);
    const target = event.currentTarget;
    const move = (e: globalThis.PointerEvent) => saveHeight(clampTerminalHeight(initial + start - e.clientY, available));
    const end = () => { target.removeEventListener('pointermove', move); target.removeEventListener('pointerup', end); target.removeEventListener('lostpointercapture', end); };
    target.addEventListener('pointermove', move); target.addEventListener('pointerup', end); target.addEventListener('lostpointercapture', end);
  };
  return <section ref={ref} id="desktop-terminal" aria-label="Terminal" className="desktop-terminal" hidden={!controller.open} style={{ height: clampTerminalHeight(height, available), '--terminal-bg': t.windowBg, '--terminal-text': t.text, '--terminal-muted': t.text3, '--terminal-border': t.border, '--terminal-hover': t.surfaceHover } as CSSProperties}>
    <div role="separator" aria-label="Terminal height" aria-orientation="horizontal" aria-valuemin={100} aria-valuemax={Math.max(100, available - 240)} aria-valuenow={clampTerminalHeight(height, available)} tabIndex={0} className="terminal-resizer" onPointerDown={resize} onKeyDown={event => { if (event.key === 'ArrowUp' || event.key === 'ArrowDown') { event.preventDefault(); saveHeight(clampTerminalHeight(height + (event.key === 'ArrowUp' ? 24 : -24), available)); } }} />
    <div className="terminal-toolbar">
      <div role="tablist" aria-label="Shell terminals" className="terminal-tabs">{controller.visibleTabs.map((tab, index) => <div className="terminal-tab" data-active={tab.id === controller.selected?.id} key={tab.id}>
        <button type="button" role="tab" aria-selected={tab.id === controller.selected?.id} aria-controls={`terminal-${tab.id}`} onClick={() => controller.select(tab.id)}><ShellIcon /><span>{tab.title}{index ? ` ${index + 1}` : ''}</span>{tab.status === 'exited' && <span className="terminal-exited-dot" aria-label="Exited">·</span>}</button>
        <button type="button" className="terminal-icon-button" aria-label={`Close ${tab.title} terminal`} title="Close terminal" onClick={() => void controller.close(tab.id)}>×</button>
      </div>)}</div>
      <button type="button" className="terminal-icon-button" disabled={controller.busy} aria-label="New terminal" title="New terminal" onClick={() => void controller.create()}>+</button>
      <div className="terminal-toolbar-spacer" />
      <button type="button" className="terminal-icon-button" aria-label="Hide terminal panel" title="Hide terminal panel (Ctrl+`)" onClick={controller.hide}>×</button>
    </div>
    {controller.error && <div role="alert" className="terminal-error">{controller.error}</div>}
    {!controller.visibleTabs.length && <div className="terminal-empty">{controller.busy ? 'Starting shell…' : <button type="button" onClick={() => void controller.create()}>Open a terminal</button>}</div>}
    <div className="terminal-surfaces">{controller.tabs.map(tab => <TerminalSurface key={tab.id} tab={tab} active={controller.open && tab.id === controller.selected?.id} focusRequest={controller.focusRequest} dark={t.dark} background={t.windowBg} />)}</div>
    {controller.selected?.status === 'exited' && <div className="terminal-exit-status">Shell exited{controller.selected.exitCode !== null ? ` (${controller.selected.exitCode})` : ''}. <button type="button" onClick={() => { const id = controller.selected!.id; void controller.close(id).then(closed => { if (closed) return controller.create(); }); }}>Restart shell</button></div>}
  </section>;
}

export function ShellIcon() { return <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="4" /><path d="m7 9 3 3-3 3m6 0h4" /></svg>; }

function TerminalSurface({ tab, active, focusRequest, dark, background }: { tab: TerminalSnapshot; active: boolean; focusRequest: { id: string; serial: number } | null; dark: boolean; background: string }) {
  const element = useRef<HTMLDivElement>(null);
  const terminal = useRef<XTerminal | null>(null);
  const fit = useRef<(() => void) | null>(null);
  const scopeRef = useRef(tab.scope); scopeRef.current = tab.scope;
  const appearance = useRef({ dark, background }); appearance.current = { dark, background };
  const activeRef = useRef(active); activeRef.current = active;
  const [error, setError] = useState<string | null>(null);
  const focus = useRef(focusRequest); focus.current = focusRequest;
  useEffect(() => {
    let disposed = false;
    let sequence = -1;
    let queued: { kind: 'output' | 'reset'; data: string; sequence: number }[] = [];
    let queuedSize = 0;
    let ready = false;
    const consume = (data: string, next: number, reset = false) => {
      if (next <= sequence) { void window.lingxi!.terminal.acknowledge(tab.id, next).catch(() => undefined); return; }
      sequence = next;
      // RIS is queued with output, preserving ordering with pending xterm writes.
      terminal.current?.write((reset ? '\x1bc' : '') + data, () => {
        if (!disposed) void window.lingxi!.terminal.acknowledge(tab.id, next).catch(() => undefined);
      });
    };
    const unsubscribe = window.lingxi!.terminal.onEvent(event => {
      if ((event.kind !== 'output' && event.kind !== 'reset') || event.terminalId !== tab.id) return;
      if (ready) consume(event.data, event.sequence, event.kind === 'reset');
      else { queued.push(event); queuedSize += event.data.length; if (queuedSize > 2_000_000) { queuedSize -= queued.shift()!.data.length; } }
    });
    let observer: ResizeObserver | undefined;
    void Promise.all([import('@xterm/xterm'), import('@xterm/addon-fit'), import('@xterm/xterm/css/xterm.css')]).then(async ([{ Terminal }, { FitAddon }]) => {
      if (disposed || !element.current) return;
      const instance = new Terminal({ cursorBlink: !window.matchMedia('(prefers-reduced-motion: reduce)').matches, fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace', fontSize: 13, lineHeight: 1.3, scrollback: 5000, theme: terminalTheme(appearance.current.dark, appearance.current.background), screenReaderMode: true, allowProposedApi: false });
      terminal.current = instance;
      const addon = new FitAddon(); instance.loadAddon(addon); instance.open(element.current);
      instance.attachCustomKeyEventHandler(event => {
        if (event.ctrlKey && event.code === 'Backquote') return false;
        if ((event.metaKey || (event.ctrlKey && event.shiftKey)) && event.code === 'KeyC' && instance.hasSelection()) { if (event.type === 'keydown') void navigator.clipboard.writeText(instance.getSelection()); return false; }
        return true;
      });
      instance.onData(data => { void window.lingxi!.terminal.input(tab.id, data).catch(reason => setError(String(reason))); });
      instance.onResize(({ cols, rows }) => { void window.lingxi!.terminal.resize(tab.id, cols, rows).catch(reason => setError(String(reason))); });
      fit.current = () => { if (activeRef.current && element.current?.clientWidth) addon.fit(); };
      observer = new ResizeObserver(() => fit.current?.()); observer.observe(element.current);
      // Subscribe before taking a fresh snapshot: discard queued chunks covered by its sequence.
      const snapshots = await window.lingxi!.terminal.list(scopeRef.current);
      if (disposed) return;
      const snapshot = snapshots.find(item => item.id === tab.id) ?? tab;
      consume(snapshot.output, snapshot.sequence);
      for (const chunk of queued) consume(chunk.data, chunk.sequence, chunk.kind === 'reset');
      queued = []; ready = true; fit.current();
      if (activeRef.current && focus.current?.id === tab.id) instance.focus();
    }).catch(reason => { if (!disposed) setError(String(reason)); });
    return () => { disposed = true; unsubscribe(); observer?.disconnect(); terminal.current?.dispose(); terminal.current = null; fit.current = null; };
  }, [tab.id]);
  useEffect(() => { if (terminal.current) terminal.current.options.theme = terminalTheme(dark, background); }, [dark, background]);
  useEffect(() => { if (active) requestAnimationFrame(() => fit.current?.()); }, [active]);
  useEffect(() => { if (active && focusRequest?.id === tab.id) terminal.current?.focus(); }, [focusRequest]);
  return <div id={`terminal-${tab.id}`} role="tabpanel" aria-label={tab.title} className="terminal-surface" hidden={!active}><div ref={element} className="terminal-emulator" />{error && <div role="alert" className="terminal-error">{error}</div>}</div>;
}

function terminalTheme(dark: boolean, background: string) {
  // xterm accepts RGB colors; resolve the application’s OKLCH tokens through canvas.
  const context = document.createElement('canvas').getContext('2d', { willReadFrequently: true });
  let resolved = dark ? '#090a10' : '#ffffff';
  if (context) { context.fillStyle = background; context.fillRect(0, 0, 1, 1); const rgba = context.getImageData(0, 0, 1, 1).data; resolved = `#${[rgba[0], rgba[1], rgba[2]].map(value => value.toString(16).padStart(2, '0')).join('')}`; }
  return dark ? { background: resolved, foreground: '#e1e1e6', cursor: '#e1e1e6', selectionBackground: '#555968', black: '#292a30', red: '#f27883', green: '#89ca8c', yellow: '#e5c07b', blue: '#79adf7', magenta: '#cc9df2', cyan: '#75cbd2', white: '#e1e1e6' } : { background: resolved, foreground: '#282a31', cursor: '#282a31', selectionBackground: '#d9e3f4', black: '#282a31', red: '#b53041', green: '#26723b', yellow: '#886015', blue: '#255daf', magenta: '#8540a7', cyan: '#16727d', white: '#737681' }; }
