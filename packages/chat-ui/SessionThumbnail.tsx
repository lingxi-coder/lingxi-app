/** @jsxRuntime automatic */
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';

type Priority = 0 | 1 | 2;
type LoadThumbnail = (signal: AbortSignal) => Promise<string>;

type ThumbnailJob = {
  visible: boolean;
  started: boolean;
  controller: AbortController;
  load: LoadThumbnail;
  complete(source: string | null): void;
};

const pending = new Set<ThumbnailJob>();
const MAX_REQUESTS = 3;
let running = 0;
let speculation: ThumbnailJob | undefined;
let flushScheduled = false;

function drainSoon() {
  if (flushScheduled) return;
  flushScheduled = true;
  queueMicrotask(() => {
    flushScheduled = false;
    while (running < MAX_REQUESTS) {
      let next: ThumbnailJob | undefined;
      for (const job of pending) {
        if (job.visible) { next = job; break; }
        if (!speculation) next ??= job;
      }
      if (!next) break;
      pending.delete(next);
      void run(next);
    }
  });
}

async function run(job: ThumbnailJob) {
  if (!job.visible) speculation = job;
  job.started = true;
  running++;
  const { signal } = job.controller;
  try {
    const source = await job.load(signal);
    if (!signal.aborted) job.complete(source);
  } catch {
    if (!signal.aborted) job.complete(null);
  } finally {
    running--;
    if (speculation === job) speculation = undefined;
    drainSoon();
  }
}

type Watch = { near: boolean; visible: boolean; update(priority: Priority): void };

// One pair of observers per scroller, including for a non-virtualized history.
const observers = new Map<Element | null, ReturnType<typeof thumbnailObservers>>();

function thumbnailObservers(root: Element | null) {
  const watches = new Map<Element, Watch>();
  function notify(entries: IntersectionObserverEntry[], field: 'near' | 'visible') {
    for (const entry of entries) {
      const watch = watches.get(entry.target);
      if (!watch || watch[field] === entry.isIntersecting) continue;
      watch[field] = entry.isIntersecting;
      watch.update(watch.visible ? 2 : watch.near ? 1 : 0);
    }
  }
  const nearby = new IntersectionObserver(entries => notify(entries, 'near'), { root, rootMargin: '320px 0px' });
  const visible = new IntersectionObserver(entries => notify(entries, 'visible'), { root });
  return { watches, nearby, visible };
}

function watchThumbnail(element: HTMLElement, update: Watch['update']) {
  if (typeof IntersectionObserver === 'undefined') {
    update(2);
    return () => {};
  }
  const root = element.closest('[data-session-image-scroll]');
  const group = observers.get(root) ?? thumbnailObservers(root);
  observers.set(root, group);
  group.watches.set(element, { near: false, visible: false, update });
  group.nearby.observe(element);
  group.visible.observe(element);
  return () => {
    group.nearby.unobserve(element);
    group.visible.unobserve(element);
    group.watches.delete(element);
    if (!group.watches.size) {
      group.nearby.disconnect();
      group.visible.disconnect();
      observers.delete(root);
    }
  };
}

interface SessionThumbnailProps {
  src?: string;
  load?: LoadThumbnail;
  alt: string;
  style?: CSSProperties;
  placeholder: ReactNode;
  loadingLabel: string;
  failedLabel: string;
}

/** A fixed frame lets the transcript paint while the thumbnail request and decode run. */
export function SessionThumbnail({ src, load, alt, style, placeholder, loadingLabel, failedLabel }: SessionThumbnailProps) {
  const frame = useRef<HTMLSpanElement>(null);
  const [windowImage, setWindowImage] = useState<{
    src?: string; load?: LoadThumbnail; priority: Priority; resolved?: string | null;
  }>();
  const [paint, setPaint] = useState<{ url: string; failed: boolean }>();
  const current = windowImage?.src === src && windowImage?.load === load ? windowImage : undefined;
  const url = current?.priority ? (load ? current.resolved : src) : undefined;
  const painted = url && paint?.url === url ? paint : undefined;
  const status = url === null || painted?.failed ? 'failed'
    : painted ? 'ready'
    : current?.priority ? 'loading' : 'idle';

  useEffect(() => {
    setWindowImage(undefined);
    setPaint(undefined);
    if (!frame.current || (!src && !load)) return;
    let priority: Priority = 0;
    let resolved: string | null | undefined;
    let request: ThumbnailJob | undefined;
    const cancel = () => {
      if (!request) return;
      pending.delete(request);
      request.controller.abort();
      request = undefined;
    };
    const report = () => setWindowImage({ src, load, priority, resolved });
    const queue = (loadThumbnail: LoadThumbnail) => {
      request = {
        visible: priority === 2,
        started: false,
        controller: new AbortController(),
        load: loadThumbnail,
        complete(source) {
          request = undefined;
          resolved = source;
          report();
        },
      };
      pending.add(request);
      drainSoon();
    };
    const unwatch = watchThumbnail(frame.current, next => {
      if (priority === next) return;
      priority = next;
      if (!next) {
        cancel();
        resolved = undefined;
        setPaint(undefined);
      } else if (request) {
        request.visible = next === 2;
        if (request.visible) {
          if (speculation === request) speculation = undefined;
        } else if (request.started) {
          if (speculation && speculation !== request) {
            const { load } = request;
            cancel();
            queue(load);
          } else {
            speculation = request;
          }
        }
        drainSoon();
      } else if (load && resolved === undefined) {
        queue(load);
      }
      report();
    });
    return () => {
      unwatch();
      cancel();
    };
  }, [src, load]);

  return <span ref={frame} className="session-image-frame" style={style} data-image-phase={status}
    data-image-visible={current?.priority === 2 || undefined}
    title={status === 'failed' ? failedLabel : status === 'loading' ? loadingLabel : undefined}>
    {status !== 'ready' && <span className="session-image-placeholder" aria-hidden="true">{placeholder}</span>}
    {url && status !== 'failed' && <img src={url} alt={alt} decoding="async" loading={current?.priority === 2 ? 'eager' : 'lazy'}
      draggable={false} onLoad={() => setPaint({ url, failed: false })} onError={() => setPaint({ url, failed: true })} />}
  </span>;
}
