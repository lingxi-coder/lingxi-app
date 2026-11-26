import { DESKTOP_BRIDGE_GLOBAL } from '../../shared/desktopBridge';
import { isHistoryImageReference, type SessionImageSize } from '../../shared/sessionHistory';
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react';
import { createPortal } from 'react-dom';
import { ComposerImageThumbnail } from '../../../../../packages/chat-ui/ComposerSurface';
import { SessionThumbnail } from '../../../../../packages/chat-ui/SessionThumbnail';
import { useI18n } from '../i18n/useI18n';
import { Icon } from './Icon';

interface ImagePreviewProps {
  src: string;
  alt: string;
  sessionId?: string;
  style?: CSSProperties;
  imageStyle?: CSSProperties;
  composerAttachment?: boolean;
}

async function loadHistoryImage(sessionId: string, reference: string, size: SessionImageSize, signal?: AbortSignal) {
  const bridge = window[DESKTOP_BRIDGE_GLOBAL];
  if (!bridge?.loadSessionImage) throw new Error('History image is unavailable');
  signal?.throwIfAborted();
  const requestId = signal && typeof bridge.cancelSessionImage === 'function' ? crypto.randomUUID() : undefined;
  const cancel = () => { if (requestId) void bridge.cancelSessionImage(requestId).catch(() => undefined); };
  signal?.addEventListener('abort', cancel, { once: true });
  try {
    const { mediaType, base64 } = await bridge.loadSessionImage(sessionId, reference, size, requestId);
    signal?.throwIfAborted();
    return `data:${mediaType};base64,${base64}`;
  } finally {
    signal?.removeEventListener('abort', cancel);
  }
}

function useOriginalImageSource(src: string, sessionId: string | undefined) {
  const [result, setResult] = useState<{ key: string; url: string | null }>();
  const historical = isHistoryImageReference(src);
  const key = historical ? JSON.stringify([sessionId, src]) : '';
  useEffect(() => {
    if (!historical || !sessionId) return;
    let cancelled = false;
    void loadHistoryImage(sessionId, src, 'original').then(
      (url) => { if (!cancelled) setResult({ key, url }); },
      () => { if (!cancelled) setResult({ key, url: null }); },
    );
    return () => { cancelled = true; };
  }, [historical, key, sessionId, src]);
  return historical ? (result?.key === key ? result.url : undefined) : src;
}

export function SessionImageThumbnail({ src, alt, sessionId, style }: Pick<ImagePreviewProps, 'src' | 'alt' | 'sessionId' | 'style'>) {
  const { tr } = useI18n();
  const historical = isHistoryImageReference(src);
  const load = useCallback((signal: AbortSignal) => loadHistoryImage(sessionId!, src, 'thumbnail', signal), [sessionId, src]);
  return <SessionThumbnail src={historical ? undefined : src} load={historical && sessionId ? load : undefined}
    alt={alt} style={style} placeholder={<Icon name="image" size={16} />}
    loadingLabel={tr('desktop_image_loading')} failedLabel={tr('desktop_image_failed')} />;
}

/** Mount the modal only while viewing; transcript thumbnails stay inexpensive. */
export function ImagePreview({ src, alt, sessionId, style, imageStyle, composerAttachment }: ImagePreviewProps) {
  const { tr } = useI18n();
  const [open, setOpen] = useState(false);
  const close = useCallback(() => setOpen(false), []);
  return <>
    <button type="button" className={composerAttachment ? 'composer-image-preview' : 'image-preview-trigger'} style={style}
      aria-label={tr('desktop_image_open', alt)} aria-haspopup="dialog" onClick={() => setOpen(true)}>
      {composerAttachment
        ? <ComposerImageThumbnail key={src} src={src} />
        : <SessionImageThumbnail src={src} alt={alt} sessionId={sessionId} style={imageStyle} />}
    </button>
    {open && createPortal(<ImageViewer key={src} source={src} sessionId={sessionId} alt={alt} onClose={close} />, document.body)}
  </>;
}

const ZOOM_STEPS = [.1, .125, .25, .5, .75, 1, 1.5, 2, 3, 4, 6, 8];
interface View { scale: number | null; x: number; y: number }
const FIT: View = { scale: null, x: 0, y: 0 };

function downloadName(src: string, alt: string): string {
  if (/\.(png|jpe?g|webp|gif|avif|svg)$/i.test(alt)) return alt;
  const extension = src.match(/^data:image\/(png|jpeg|webp|gif|avif|svg\+xml)[;,]/i)?.[1];
  return `${alt || 'image'}${extension ? `.${extension === 'svg+xml' ? 'svg' : extension === 'jpeg' ? 'jpg' : extension}` : ''}`;
}

function ImageViewer({ source, sessionId, alt, onClose }: { source: string; sessionId?: string; alt: string; onClose(): void }) {
  const { tr } = useI18n();
  const src = useOriginalImageSource(source, sessionId);
  const dialog = useRef<HTMLDialogElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const drag = useRef<{ id: number; startX: number; startY: number; x: number; y: number } | null>(null);
  const ignoreBackdropClick = useRef(false);
  const [dragging, setDragging] = useState(false);
  const [natural, setNatural] = useState({ width: 0, height: 0 });
  const [bounds, setBounds] = useState({ width: 0, height: 0 });
  const [saving, setSaving] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);
  const [failed, setFailed] = useState(false);
  const [view, setView] = useState<View>(FIT);
  const ready = natural.width > 0 && bounds.width > 0 && !failed;
  const fit = ready ? Math.min(1, bounds.width / natural.width, bounds.height / natural.height) : 1;
  const scale = view.scale ?? fit;
  const minimum = Math.min(.1, fit);
  const limitX = Math.max(0, (natural.width * scale - bounds.width) / 2);
  const limitY = Math.max(0, (natural.height * scale - bounds.height) / 2);
  const x = Math.max(-limitX, Math.min(limitX, view.x));
  const y = Math.max(-limitY, Math.min(limitY, view.y));
  const pannable = ready && (limitX > 0 || limitY > 0);

  useLayoutEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const overflow = document.body.style.overflow;
    element.showModal();
    closeButton.current?.focus({ preventScroll: true });
    document.body.style.overflow = 'hidden';
    return () => {
      element.close();
      document.body.style.overflow = overflow;
      if (previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, []);

  useLayoutEffect(() => {
    const element = viewport.current!;
    const resize = () => {
      setBounds({ width: element.clientWidth, height: element.clientHeight });
      setView(FIT);
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Zoom around the pointer, preserving the content beneath it. Button/key zoom
  // uses the center. No transform transitions: dragging always follows input 1:1.
  const zoom = useCallback((requested: number | 'in' | 'out' | ((scale: number) => number), anchor = { x: 0, y: 0 }) => {
    if (!ready) return;
    setView((current) => {
      const oldScale = current.scale ?? fit;
      const target = requested === 'in' ? ZOOM_STEPS.find((step) => step > oldScale + .001) ?? 8
        : requested === 'out' ? [...ZOOM_STEPS].reverse().find((step) => step < oldScale - .001) ?? minimum
          : typeof requested === 'function' ? requested(oldScale) : requested;
      const nextScale = Math.max(minimum, Math.min(8, target));
      const oldLimitX = Math.max(0, (natural.width * oldScale - bounds.width) / 2);
      const oldLimitY = Math.max(0, (natural.height * oldScale - bounds.height) / 2);
      const oldX = Math.max(-oldLimitX, Math.min(oldLimitX, current.x));
      const oldY = Math.max(-oldLimitY, Math.min(oldLimitY, current.y));
      const nextLimitX = Math.max(0, (natural.width * nextScale - bounds.width) / 2);
      const nextLimitY = Math.max(0, (natural.height * nextScale - bounds.height) / 2);
      return {
        scale: nextScale,
        x: Math.max(-nextLimitX, Math.min(nextLimitX, anchor.x - (anchor.x - oldX) * nextScale / oldScale)),
        y: Math.max(-nextLimitY, Math.min(nextLimitY, anchor.y - (anchor.y - oldY) * nextScale / oldScale)),
      };
    });
  }, [ready, fit, minimum, natural.width, natural.height, bounds.width, bounds.height]);

  useEffect(() => {
    const element = viewport.current!;
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = element.getBoundingClientRect();
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? bounds.height : 1);
      const factor = Math.exp(-Math.max(-100, Math.min(100, delta)) * (event.ctrlKey ? .01 : .002));
      // Functional updates keep rapid wheel events from using a stale zoom value.
      zoom((current) => current * factor, { x: event.clientX - rect.left - rect.width / 2, y: event.clientY - rect.top - rect.height / 2 });
    };
    element.addEventListener('wheel', wheel, { passive: false });
    return () => element.removeEventListener('wheel', wheel);
  }, [zoom, bounds.height]);

  const stopDrag = (pointerId: number) => {
    if (drag.current?.id !== pointerId) return;
    drag.current = null;
    setDragging(false);
  };

  return <dialog ref={dialog} className="image-viewer" aria-label={tr('desktop_image_viewer')} aria-modal="true"
    onCancel={(event) => { event.preventDefault(); event.stopPropagation(); onClose(); }}
    onClick={(event) => { if (event.target === event.currentTarget) onClose(); }}
    onKeyDown={(event) => {
      event.stopPropagation();
      if (event.nativeEvent.isComposing) return;
      if (event.key === '+' || event.key === '=') { event.preventDefault(); zoom('in'); }
      else if (event.key === '-') { event.preventDefault(); zoom('out'); }
      else if (event.key === '0') { event.preventDefault(); setView(FIT); }
      else if (event.key === '1') { event.preventDefault(); zoom(1); }
      else if (pannable && ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) {
        event.preventDefault();
        const dx = event.key === 'ArrowLeft' ? 64 : event.key === 'ArrowRight' ? -64 : 0;
        const dy = event.key === 'ArrowUp' ? 64 : event.key === 'ArrowDown' ? -64 : 0;
        setView({ scale, x: Math.max(-limitX, Math.min(limitX, x + dx)), y: Math.max(-limitY, Math.min(limitY, y + dy)) });
      }
    }}>
    <div className="image-viewer-actions">
      <button type="button" className="image-viewer-control" disabled={saving || !ready} aria-busy={saving}
        onClick={async () => { if (!src) return; setSaving(true); setSaveFailed(false); try { await window[DESKTOP_BRIDGE_GLOBAL]!.saveImage({ dataUrl: src, suggestedName: downloadName(src, alt) }); } catch { setSaveFailed(true); } finally { setSaving(false); } }}
        aria-label={tr('desktop_image_download')} title={tr('desktop_image_download')}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M12 3v12m-4-4 4 4 4-4M5 15v4a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-4" /></svg>
      </button>
      <button ref={closeButton} type="button" autoFocus className="image-viewer-control" aria-label={tr('desktop_image_close')} title={tr('desktop_image_close')}
        onClick={onClose}><Icon name="x" size={22} /></button>
    </div>
    <div ref={viewport} className="image-viewer-viewport" data-pannable={pannable} data-dragging={dragging}
      onPointerDown={(event) => {
        ignoreBackdropClick.current = false;
        if (event.target === event.currentTarget || event.button !== 0) return;
        ignoreBackdropClick.current = true;
        if (!pannable) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { id: event.pointerId, startX: event.clientX, startY: event.clientY, x, y };
        setDragging(true);
      }}
      onPointerMove={(event) => {
        const current = drag.current;
        if (current?.id !== event.pointerId) return;
        setView({ scale, x: Math.max(-limitX, Math.min(limitX, current.x + event.clientX - current.startX)), y: Math.max(-limitY, Math.min(limitY, current.y + event.clientY - current.startY)) });
      }}
      onPointerUp={(event) => stopDrag(event.pointerId)}
      onPointerCancel={(event) => stopDrag(event.pointerId)}
      onLostPointerCapture={(event) => stopDrag(event.pointerId)}
      onClick={(event) => { if (event.target === event.currentTarget && !ignoreBackdropClick.current) onClose(); }}
      onDoubleClick={(event) => {
        if (event.target === event.currentTarget || !ready) return;
        if (scale > fit + .001) setView(FIT);
        else {
          const rect = event.currentTarget.getBoundingClientRect();
          zoom(fit < 1 ? 1 : 2, { x: event.clientX - rect.left - rect.width / 2, y: event.clientY - rect.top - rect.height / 2 });
        }
      }}>
      {!ready && <div className={failed || src === null ? 'image-viewer-status' : 'image-viewer-status session-image-loading-status'}
        role={failed || src === null ? 'alert' : 'status'}>{tr(failed || src === null ? 'desktop_image_failed' : 'desktop_image_loading')}</div>}
      {src && <img className="image-viewer-image" src={src} alt={alt} decoding="async" draggable={false}
        onLoad={(event) => { setNatural({ width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight }); }}
        onError={() => setFailed(true)}
        style={{ width: natural.width || undefined, height: natural.height || undefined, visibility: ready ? 'visible' : 'hidden', transform: `translate(${x}px, ${y}px) scale(${scale})` }} />}
    </div>
    {saveFailed && <div className="image-viewer-status" role="alert">{tr('image_preview_save_failed')}</div>}
    <div className="image-viewer-zoom" role="group" aria-label={tr('desktop_image_zoom')}>
      <button type="button" className="image-viewer-control" disabled={!ready || scale <= minimum + .001}
        aria-label={tr('desktop_image_zoom_out')} title={tr('desktop_image_zoom_out')} onClick={() => zoom('out')}><Icon name="minus" size={20} /></button>
      <button type="button" className="image-viewer-percentage" disabled={!ready} aria-label={tr('desktop_image_fit')}
        title={tr('desktop_image_fit')} onClick={() => setView(FIT)}>{ready ? `${Math.round(scale * 100)}%` : '—'}</button>
      <button type="button" className="image-viewer-control" disabled={!ready || scale >= 8}
        aria-label={tr('desktop_image_zoom_in')} title={tr('desktop_image_zoom_in')} onClick={() => zoom('in')}><Icon name="plus" size={20} /></button>
    </div>
  </dialog>;
}
