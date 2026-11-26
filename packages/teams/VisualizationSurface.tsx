import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { VisualizationBundle } from './visualization.js';

export type SurfaceValue = { type: string; [key: string]: unknown };
export interface SurfaceRequest extends SurfaceValue { type: 'request'; id: number; action: string }
const surfaceDocument = new URL('../../resources/team-visualization/assets/team-visualization.html', import.meta.url).href;

interface Props {
  bundle: VisualizationBundle;
  aliases: string[];
  locale: string;
  expanded?: boolean;
  initialState?: unknown;
  channel?: { current?: (message: SurfaceValue) => void };
  request(message: SurfaceRequest): Promise<unknown>;
  onFailure(code: string): void;
}

/** The iframe receives only our own shell. Author code runs inside its offline VM. */
export function VisualizationSurface({ bundle, aliases, locale, expanded = false, initialState, channel, request, onFailure }: Props) {
  const iframe = useRef<HTMLIFrameElement>(null);
  const connection = useRef<MessagePort>();
  const latest = useRef({ request, onFailure, expanded });
  latest.current = { request, onFailure, expanded };
  const [height, setHeight] = useState(40);
  const [rendered, setRendered] = useState(false);
  useLayoutEffect(() => {
    let live = true;
    let ready = false;
    const dark = window.matchMedia('(prefers-color-scheme: dark)');
    const theme = () => ({ mode: dark.matches ? 'dark' : 'light', tokens: {} });
    const updateTheme = () => connection.current?.postMessage({ type: 'layout', value: {
      width: iframe.current?.clientWidth ?? 0, theme: theme(), expanded: latest.current.expanded,
    } });
    const sourceSize = Object.values(bundle.assets ?? {}).reduce((size, asset) => size + asset.data.length, bundle.html.length);
    const timeout = window.setTimeout(() => latest.current.onFailure('VISUALIZATION_SCRIPT_FAILED'),
      sourceSize > 2 * 1024 * 1024 ? 50_000 : 10_000);
    const connect = (event: MessageEvent) => {
      const viewer = iframe.current?.contentWindow;
      if (!viewer || event.source !== viewer || event.origin !== 'null' || event.data?.type !== 'teamVisualizationReady' || connection.current) return;
      const { port1, port2 } = new MessageChannel();
      connection.current = port1;
      if (channel) channel.current = value => port1.postMessage(value);
      port1.onmessage = ({ data }: MessageEvent<SurfaceValue>) => {
        if (!live || !data || typeof data !== 'object') return;
        if (data.type === 'ready' && !ready) {
          ready = true;
          port1.postMessage({ type: 'initialize', value: { bundle, aliases, locale, state: initialState, theme: theme(),
            width: iframe.current?.clientWidth ?? 0, expanded: latest.current.expanded } });
        } else if (data.type === 'height' && Number.isFinite(data.height)) {
          setHeight(Math.min(4096, Math.max(32, data.height as number)));
        } else if (data.type === 'rendered') {
          window.clearTimeout(timeout);
          setRendered(true);
        } else if (data.type === 'error') {
          latest.current.onFailure(String(data.code ?? 'VISUALIZATION_SCRIPT_FAILED'));
        } else if (ready && data.type === 'request' && Number.isSafeInteger(data.id) && typeof data.action === 'string') {
          const answer = (value: SurfaceValue) => { if (live) port1.postMessage({ type: 'result', value: { id: data.id, ...value } }); };
          void latest.current.request(data as SurfaceRequest).then(value => answer({ type: 'ok', value }),
            error => answer({ type: 'error', error: error instanceof Error ? error.message : 'DATA_UNAVAILABLE' }));
        }
      };
      // Only the port is transferred to the verified opaque frame. All other
      // communication goes over that port after the frame acknowledges it.
      viewer.postMessage({ type: 'connectTeamVisualization' }, '*', [port2]);
    };
    window.addEventListener('message', connect);
    dark.addEventListener('change', updateTheme);
    const element = iframe.current;
    if (element) element.src = surfaceDocument;
    const resize = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(updateTheme);
    if (resize && element) resize.observe(element);
    return () => {
      live = false;
      window.clearTimeout(timeout);
      dark.removeEventListener('change', updateTheme);
      window.removeEventListener('message', connect);
      resize?.disconnect();
      connection.current?.postMessage({ type: 'suspend' });
      connection.current?.close();
      connection.current = undefined;
      if (channel) channel.current = undefined;
    };
  }, [bundle, aliases, locale, initialState, channel]);
  useEffect(() => {
    connection.current?.postMessage({ type: 'layout', value: { width: iframe.current?.clientWidth ?? 0, expanded } });
  }, [expanded]);
  return <iframe ref={iframe} title={bundle.title} sandbox="allow-scripts"
    referrerPolicy="no-referrer" className="tw-visualization-frame" aria-busy={!rendered}
    style={{ height: Math.min(height, expanded ? 960 : 540) }} />;
}
