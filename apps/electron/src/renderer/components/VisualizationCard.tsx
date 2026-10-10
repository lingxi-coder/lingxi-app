/**
 * One inline visualization in the transcript.
 *
 * The widget runs in a hardened `<webview>` (see `main/visualization.ts`):
 * its main frame is the engine's trusted shell, the author fragment sits one
 * frame deeper in an opaque-origin sandbox. This component is the shell's
 * only peer: it authorizes the mount through the main process, relays state
 * writes (compare-and-swap in the engine), sizes the webview, and turns
 * follow-up drafts into composer text. Expanding reuses the SAME webview as a
 * fixed overlay, so a widget never runs twice.
 */

import { memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import type { WebviewTag } from 'electron';

import type { VisualizationContextChip, VisualizationFollowup } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import type { Tokens } from '../theme/tokens';
import {
  MAX_ACTIVE_VISUALIZATIONS,
  MAX_GUEST_MESSAGE_CHARS,
  VISUALIZATION_GUEST_CHANNEL,
  VISUALIZATION_MAX_INLINE_HEIGHT,
  VISUALIZATION_MIN_HEIGHT,
  VISUALIZATION_PARTITION,
  VISUALIZATION_SHELL_URL,
  type VisualizationMount,
  type VisualizationReference,
  type VisualizationTheme,
} from '../../shared/visualization';

export type { VisualizationFollowup };

/** Map the app palette onto the upstream variables `visualize.css` reads. */
export function visualizationTheme(t: Tokens): VisualizationTheme {
  return {
    dark: t.dark,
    tokens: {
      '--color-background-primary': t.transcriptBg,
      '--color-background-secondary': t.surface,
      '--color-background-info': t.accentBg,
      '--color-text-primary': t.text,
      '--color-text-secondary': t.text2,
      '--color-text-info': t.accent,
      '--color-text-inverse': t.dark ? '#0d0d0d' : '#ffffff',
      '--color-text-warning': t.warn,
      '--color-border-primary': t.borderStrong,
      '--color-border-secondary': t.border,
      '--color-ring-primary': t.accent,
    },
  };
}

/**
 * Least-recently-visible eviction across every card: at most
 * {@link MAX_ACTIVE_VISUALIZATIONS} webviews run at once.
 */
const activeCards = new Map<symbol, { lastVisible: number; release: () => void }>();

function claimSlot(key: symbol, release: () => void): void {
  activeCards.set(key, { lastVisible: Date.now(), release });
  while (activeCards.size > MAX_ACTIVE_VISUALIZATIONS) {
    let oldest: [symbol, { lastVisible: number; release: () => void }] | undefined;
    for (const entry of activeCards) {
      if (entry[0] !== key && (!oldest || entry[1].lastVisible < oldest[1].lastVisible)) oldest = entry;
    }
    if (!oldest) break;
    activeCards.delete(oldest[0]);
    oldest[1].release();
  }
}

function touchSlot(key: symbol): void {
  const entry = activeCards.get(key);
  if (entry) entry.lastVisible = Date.now();
}

type ShellMessage = Record<string, unknown> & { type?: unknown; generation?: unknown };

function parseShellMessage(raw: unknown): ShellMessage | null {
  if (typeof raw !== 'string' || raw.length > MAX_GUEST_MESSAGE_CHARS) return null;
  try {
    const value: unknown = JSON.parse(raw);
    return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as ShellMessage : null;
  } catch {
    return null;
  }
}

interface CardProps {
  readonly sessionId: string;
  readonly reference: VisualizationReference;
  readonly onFollowup?: (followup: VisualizationFollowup) => void;
  /** Open in the expanded overlay as soon as it mounts (tool-row "Open"). */
  readonly startExpanded?: boolean;
  readonly onClose?: () => void;
}

type Phase = 'idle' | 'loading' | 'ready' | 'unavailable' | 'crashed';

export const VisualizationCard = memo(function VisualizationCard({ sessionId, reference, onFollowup, startExpanded = false, onClose }: CardProps) {
  const t = useT();
  const theme = useMemo(() => visualizationTheme(t), [t]);
  const container = useRef<HTMLDivElement | null>(null);
  const webview = useRef<WebviewTag | null>(null);
  const mount = useRef<VisualizationMount | null>(null);
  const slot = useMemo(() => Symbol('visualization'), []);
  const [visible, setVisible] = useState(startExpanded);
  const [active, setActive] = useState(startExpanded);
  const [phase, setPhase] = useState<Phase>('idle');
  const [height, setHeight] = useState(VISUALIZATION_MIN_HEIGHT * 4);
  const [clamped, setClamped] = useState(false);
  const [expanded, setExpanded] = useState(startExpanded);
  const [title, setTitle] = useState('');
  const [attempt, setAttempt] = useState(0);
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  const themeRef = useRef(theme);
  themeRef.current = theme;

  const post = useCallback((message: Record<string, unknown>) => {
    const view = webview.current;
    if (!view) return;
    try {
      view.send(VISUALIZATION_GUEST_CHANNEL, JSON.stringify(message));
    } catch {
      // The guest is gone; the crash path remounts it.
    }
  }, []);

  const retire = useCallback(() => {
    const current = mount.current;
    mount.current = null;
    if (current) void window.lingxi?.visualization.unmount(sessionId, current.token).catch(() => undefined);
  }, [sessionId]);

  // Mount only while near the viewport.
  useEffect(() => {
    if (startExpanded) return;
    const node = container.current;
    if (!node) return;
    const observer = new IntersectionObserver((entries) => {
      const isVisible = entries.some((entry) => entry.isIntersecting);
      setVisible(isVisible);
      if (isVisible) setActive(true);
    }, { rootMargin: '600px 0px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, [startExpanded]);

  useEffect(() => {
    if (!active) return;
    claimSlot(slot, () => {
      post({ type: 'suspend', generation: mount.current?.generation ?? 0 });
      window.setTimeout(() => {
        retire();
        setActive(false);
        setPhase('idle');
      }, 1_600);
    });
    return () => {
      activeCards.delete(slot);
    };
  }, [active, slot, post, retire]);

  useEffect(() => {
    if (visible) touchSlot(slot);
  }, [visible, slot]);

  // Theme follows the app without remounting.
  useEffect(() => {
    if (mount.current) post({ type: 'theme', generation: mount.current.generation, theme });
  }, [theme, post]);

  const setExpandedState = useCallback((next: boolean) => {
    setExpanded(next);
    if (mount.current) post({ type: 'expanded', generation: mount.current.generation, expanded: next });
    if (!next) onClose?.();
  }, [post, onClose]);

  const handleShellMessage = useCallback(async (message: ShellMessage) => {
    const current = mount.current;
    if (message.type === 'shell.ready') {
      retire();
      setPhase('loading');
      const ticket = await (window.lingxi?.visualization.mount(
        sessionId, reference, themeRef.current, navigator.language || 'en', expandedRef.current,
      ) ?? Promise.resolve(null)).catch(() => null);
      if (!ticket) {
        setPhase('unavailable');
        return;
      }
      mount.current = ticket;
      setTitle(ticket.title);
      post({
        type: 'mount',
        generation: ticket.generation,
        docUrl: ticket.docUrl,
        title: ticket.title,
        maxHeight: VISUALIZATION_MAX_INLINE_HEIGHT,
        expanded: expandedRef.current,
      });
      return;
    }
    if (!current || message.generation !== current.generation) return;
    switch (message.type) {
      case 'ready':
        setPhase('ready');
        break;
      case 'resize':
        if (typeof message.height === 'number' && Number.isFinite(message.height)) {
          setHeight(Math.max(VISUALIZATION_MIN_HEIGHT, Math.min(VISUALIZATION_MAX_INLINE_HEIGHT, Math.ceil(message.height))));
          setClamped(message.clamped === true);
        }
        break;
      case 'state.save': {
        const { requestId, baseVersion, modelContent, privateContent } = message;
        if (!Number.isSafeInteger(requestId) || !Number.isSafeInteger(baseVersion)
          || typeof modelContent !== 'string' || typeof privateContent !== 'string') return;
        const write = window.lingxi?.visualization.writeState(
          sessionId, current.token, current.generation, baseVersion as number, modelContent, privateContent,
        ) ?? Promise.reject(new Error('host unavailable'));
        const result = await write.catch(() => ({ saved: false, version: 0, reason: 'unavailable' as string }));
        if (mount.current !== current) return;
        post(result.saved
          ? { type: 'state.saved', generation: current.generation, requestId, version: result.version }
          : { type: 'state.rejected', generation: current.generation, requestId, reason: result.reason ?? 'rejected', state: 'currentState' in result ? result.currentState ?? null : null });
        break;
      }
      case 'followup.draft':
        if (typeof message.text === 'string' && message.text.trim()) {
          onFollowup?.({ text: message.text, reference, title: current.title });
        }
        break;
      case 'expand.request':
        setExpandedState(true);
        break;
      case 'crashed':
        setPhase('crashed');
        break;
      case 'suspended':
      case 'error':
      default:
        break;
    }
  }, [sessionId, reference, post, retire, onFollowup, setExpandedState]);

  // Wire the guest once it is in the DOM.
  useEffect(() => {
    const view = webview.current;
    if (!view || !active) return;
    let guestId: number | null = null;
    const onMessage = (event: Electron.IpcMessageEvent) => {
      if (event.channel !== VISUALIZATION_GUEST_CHANNEL) return;
      const message = parseShellMessage(event.args[0]);
      if (message) void handleShellMessage(message);
    };
    const onDomReady = () => {
      try {
        guestId = view.getWebContentsId();
      } catch {
        guestId = null;
      }
    };
    const onFail = () => setPhase('crashed');
    view.addEventListener('ipc-message', onMessage);
    view.addEventListener('dom-ready', onDomReady);
    view.addEventListener('did-fail-load', onFail);
    const unsubscribe = window.lingxi?.visualization.onGuestEvent((event) => {
      if (guestId === null || event.webContentsId !== guestId) return;
      if (event.reason === 'crashed') setPhase('crashed');
      if (event.reason === 'escape' && expandedRef.current) setExpandedState(false);
    });
    return () => {
      view.removeEventListener('ipc-message', onMessage);
      view.removeEventListener('dom-ready', onDomReady);
      view.removeEventListener('did-fail-load', onFail);
      unsubscribe?.();
    };
  }, [active, attempt, handleShellMessage, setExpandedState]);

  // A crashed or reloaded widget restarts from its confirmed state.
  useEffect(() => {
    if (phase !== 'crashed') return;
    retire();
    const timer = window.setTimeout(() => setAttempt((value) => value + 1), 400);
    return () => window.clearTimeout(timer);
  }, [phase, retire]);

  useEffect(() => () => {
    post({ type: 'suspend', generation: mount.current?.generation ?? 0 });
    retire();
  }, [post, retire]);

  const frameStyle: CSSProperties = expanded
    ? { position: 'fixed', inset: '48px 32px 32px', zIndex: 60, height: 'auto', borderRadius: 14, boxShadow: t.windowShadow, background: t.transcriptBg }
    : { height, width: '100%', borderRadius: 12 };

  if (phase === 'unavailable') {
    return (
      <div className="visualization-card visualization-unavailable" role="note"
        style={{ border: `1px solid ${t.border}`, borderRadius: 12, padding: '10px 12px', color: t.text3, fontSize: 13 }}>
        Visualization unavailable in this conversation.
      </div>
    );
  }

  return (
    <div ref={container} className="visualization-card" data-visualization-id={reference.id}
      aria-label={title ? `Visualization: ${title}` : 'Visualization'} role="group"
      style={{ position: 'relative', width: '100%', minHeight: VISUALIZATION_MIN_HEIGHT }}>
      {expanded && <div className="visualization-backdrop" onClick={() => setExpandedState(false)}
        style={{ position: 'fixed', inset: 0, zIndex: 59, background: t.dark ? 'rgba(0,0,0,0.55)' : 'rgba(0,0,0,0.25)' }} />}
      {active ? (
        <webview
          key={attempt}
          ref={(node: WebviewTag | null) => { webview.current = node; }}
          src={VISUALIZATION_SHELL_URL}
          partition={VISUALIZATION_PARTITION}
          style={{ display: 'flex', border: 0, overflow: 'hidden', ...frameStyle }}
        />
      ) : (
        <div style={{ height, borderRadius: 12, background: t.surface }} aria-hidden="true" />
      )}
      {phase !== 'ready' && active && (
        <div aria-live="polite" style={{ position: 'absolute', left: 12, top: 10, color: t.text3, fontSize: 12 }}>
          {phase === 'crashed' ? 'Reloading visualization…' : 'Loading visualization…'}
        </div>
      )}
      {(clamped || expanded) && phase === 'ready' && (
        <button type="button" className="visualization-expand"
          onClick={() => setExpandedState(!expanded)}
          aria-label={expanded ? 'Close expanded visualization' : 'Expand visualization'}
          style={{ position: expanded ? 'fixed' : 'absolute', right: expanded ? 40 : 8, top: expanded ? 56 : 8, zIndex: 61,
            border: `1px solid ${t.border}`, borderRadius: 8, background: t.surface, color: t.text2, fontSize: 12, padding: '3px 8px', cursor: 'pointer' }}>
          {expanded ? 'Close' : 'Expand'}
        </button>
      )}
    </div>
  );
});

/** A slot whose reference line is still streaming. */
export function VisualizationPlaceholder() {
  const t = useT();
  return (
    <div className="visualization-card visualization-pending" aria-busy="true"
      style={{ height: 96, borderRadius: 12, background: t.surface, color: t.text3, fontSize: 12, padding: '10px 12px' }}>
      Preparing visualization…
    </div>
  );
}

/** A widget whose revision could not be published or was removed. */
export function VisualizationUnavailable() {
  const t = useT();
  return (
    <div className="visualization-card visualization-unavailable" role="note"
      style={{ borderRadius: 12, border: `1px dashed ${t.border}`, color: t.text3, fontSize: 12, padding: '10px 12px' }}>
      This visualization is no longer available.
    </div>
  );
}

/** The widget a user message followed up on, shown above its bubble. */
export function VisualizationContextBadge({ chip, onDismiss }: { chip: VisualizationContextChip; onDismiss?: () => void }) {
  const t = useT();
  return (
    <span className="visualization-context-chip" title={`About visualization: ${chip.title}`}
      style={{ display: 'inline-flex', alignItems: 'center', gap: 6, maxWidth: 320, borderRadius: 999, border: `1px solid ${t.border}`,
        background: t.surface, color: t.text2, fontSize: 12, padding: '2px 8px' }}>
      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{chip.title || 'Visualization'}</span>
      {onDismiss && (
        <button type="button" onClick={onDismiss} aria-label="Remove visualization context"
          style={{ border: 0, background: 'transparent', color: t.text3, cursor: 'pointer', padding: 0, fontSize: 12 }}>×</button>
      )}
    </span>
  );
}
