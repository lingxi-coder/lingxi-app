/**
 * Inline visualization host for the Electron desktop.
 *
 * - {@link VisualizationRouter} answers the `lingxi-viz://visualization` scheme
 *   and the renderer's mount/state IPC by forwarding to the bridge-server that
 *   owns the conversation. A mount token is valid only on the bridge that
 *   issued it, so documents and state writes route by token owner; the shell
 *   and library assets are immutable and cached after the first fetch.
 * - {@link guardVisualizationWebview} is the ONLY way a `<webview>` attaches:
 *   it must target the shell on the in-memory visualization partition, and
 *   its web preferences are overwritten, never trusted from the renderer.
 * - {@link installVisualizationSession} locks that partition down: only this
 *   scheme loads, a black-hole proxy (loopback included) swallows anything
 *   else including WebRTC, every permission and download is refused.
 * - {@link hardenVisualizationGuest} denies pop-ups and navigation away from
 *   the scheme, and reports crashes and Escape to the embedding renderer.
 *
 * Only type imports from `electron` here, so the routing logic runs under
 * plain Node tests.
 */

import type { Event, Session, WebContents, WebPreferences } from 'electron';
import type {
  VisualizationMountDto,
  VisualizationServeDto,
  VisualizationStateWriteDto,
  VisualizationThemeDto,
} from '@lingxi/bridge-client';

import {
  VISUALIZATION_ORIGIN,
  VISUALIZATION_PARTITION,
  VISUALIZATION_SCHEME,
  VISUALIZATION_SHELL_URL,
  type VisualizationGuestEvent,
  type VisualizationMount,
  type VisualizationReference,
  type VisualizationStateWrite,
} from '../shared/visualization.js';

/** What one session runtime offers the router (implemented by `SessionRuntime`). */
export interface VisualizationBackend {
  visualizationMount(
    reference: VisualizationReference,
    theme: VisualizationThemeDto,
    locale: string,
    expanded: boolean,
  ): Promise<VisualizationMountDto | null>;
  visualizationServe(path: string): Promise<VisualizationServeDto>;
  visualizationWriteState(
    token: string,
    generation: number,
    baseVersion: number,
    modelContent: string,
    privateContent: string,
  ): Promise<VisualizationStateWriteDto>;
  visualizationUnmount(token: string): Promise<void>;
}

export interface VisualizationBackends {
  /** The runtime of a live session, if any. */
  get(sessionId: string): VisualizationBackend | undefined;
  /** Any connected runtime, for the immutable shell and library assets. */
  any(): VisualizationBackend | undefined;
}

export interface ServedVisualization {
  readonly status: number;
  readonly headers: ReadonlyArray<readonly [string, string]>;
  readonly body: Buffer;
}

const DOC_PREFIX = '/doc/';
const STATIC_PATHS = /^\/(shell\.(html|js|css)|asset\/[A-Za-z0-9._-]+\.js)$/;

function notFound(): ServedVisualization {
  return { status: 404, headers: [['Content-Type', 'text/plain; charset=utf-8'], ['Cache-Control', 'no-store']], body: Buffer.from('not found') };
}

/** The path of a `lingxi-viz://visualization/...` URL, or `null` for anything else. */
export function visualizationPath(raw: string): string | null {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol !== `${VISUALIZATION_SCHEME}:` || url.host !== 'visualization') return null;
  if (url.username || url.password || url.port || url.search || url.hash) return null;
  return url.pathname;
}

export class VisualizationRouter {
  private readonly owners = new Map<string, string>();
  private readonly staticResponses = new Map<string, Promise<ServedVisualization>>();

  constructor(private readonly backends: VisualizationBackends) {}

  async mount(
    sessionId: string,
    reference: VisualizationReference,
    theme: VisualizationThemeDto,
    locale: string,
    expanded: boolean,
  ): Promise<VisualizationMount | null> {
    const backend = this.backends.get(sessionId);
    if (!backend) return null;
    const ticket = await backend.visualizationMount(reference, theme, locale, expanded);
    if (!ticket || !ticket.doc_url.startsWith(`${VISUALIZATION_ORIGIN}${DOC_PREFIX}`)) return null;
    this.owners.set(ticket.token, sessionId);
    return { token: ticket.token, generation: ticket.generation, docUrl: ticket.doc_url, title: ticket.title };
  }

  async writeState(
    sessionId: string,
    token: string,
    generation: number,
    baseVersion: number,
    modelContent: string,
    privateContent: string,
  ): Promise<VisualizationStateWrite> {
    const backend = this.owners.get(token) === sessionId ? this.backends.get(sessionId) : undefined;
    if (!backend) return { saved: false, version: 0, reason: 'stale_mount' };
    const result = await backend.visualizationWriteState(token, generation, baseVersion, modelContent, privateContent);
    return {
      saved: result.saved,
      version: result.version,
      ...(result.reason === undefined ? {} : { reason: result.reason }),
      ...(result.current_state === undefined ? {} : { currentState: result.current_state }),
    };
  }

  async unmount(sessionId: string, token: string): Promise<void> {
    if (this.owners.get(token) !== sessionId) return;
    this.owners.delete(token);
    await this.backends.get(sessionId)?.visualizationUnmount(token).catch(() => undefined);
  }

  /** Forget every mount of a session whose runtime went away. */
  forgetSession(sessionId: string): void {
    for (const [token, owner] of this.owners) {
      if (owner === sessionId) this.owners.delete(token);
    }
  }

  /** Answer one request of the visualization scheme. */
  async serve(rawUrl: string): Promise<ServedVisualization> {
    const path = visualizationPath(rawUrl);
    if (path === null) return notFound();
    if (path.startsWith(DOC_PREFIX)) {
      const token = path.slice(DOC_PREFIX.length);
      const owner = this.owners.get(token);
      const backend = owner === undefined ? undefined : this.backends.get(owner);
      return backend ? decode(await backend.visualizationServe(path)) : notFound();
    }
    if (!STATIC_PATHS.test(path)) return notFound();
    let cached = this.staticResponses.get(path);
    if (!cached) {
      const backend = this.backends.any();
      if (!backend) return notFound();
      cached = backend.visualizationServe(path).then(decode);
      this.staticResponses.set(path, cached);
      cached.then((response) => {
        if (response.status !== 200) this.staticResponses.delete(path);
      }, () => this.staticResponses.delete(path));
    }
    return cached;
  }
}

function decode(served: VisualizationServeDto): ServedVisualization {
  return { status: served.status, headers: served.headers, body: Buffer.from(served.body_base64, 'base64') };
}

/** Web preferences every visualization guest gets, whatever the renderer asked for. */
export function visualizationGuestPreferences(preload: string, devTools: boolean): WebPreferences {
  return {
    preload,
    sandbox: true,
    contextIsolation: true,
    nodeIntegration: false,
    nodeIntegrationInSubFrames: false,
    nodeIntegrationInWorker: false,
    webSecurity: true,
    allowRunningInsecureContent: false,
    webviewTag: false,
    plugins: false,
    experimentalFeatures: false,
    navigateOnDragDrop: false,
    spellcheck: false,
    safeDialogs: true,
    autoplayPolicy: 'user-gesture-required',
    devTools,
  };
}

/**
 * `will-attach-webview` handler: refuse every webview except the
 * visualization shell on its partition, and overwrite its preferences.
 */
export function guardVisualizationWebview(
  event: Pick<Event, 'preventDefault'>,
  webPreferences: WebPreferences & { preloadURL?: string },
  params: Record<string, string>,
  preload: string,
  devTools: boolean,
): boolean {
  const allowed = params['src'] === VISUALIZATION_SHELL_URL
    && params['partition'] === VISUALIZATION_PARTITION
    && !params['allowpopups']
    && !params['nodeintegration']
    && !params['nodeintegrationinsubframes']
    && !params['plugins']
    && !params['disablewebsecurity'];
  if (!allowed) {
    event.preventDefault();
    return false;
  }
  delete webPreferences.preloadURL;
  for (const key of Object.keys(webPreferences)) delete (webPreferences as Record<string, unknown>)[key];
  Object.assign(webPreferences, visualizationGuestPreferences(preload, devTools));
  return true;
}

function sameScheme(raw: string): boolean {
  return visualizationPath(raw) !== null;
}

/** Harden one attached guest and forward its crashes and Escape key to the embedder. */
export function hardenVisualizationGuest(
  guest: WebContents,
  notify: (event: VisualizationGuestEvent) => void,
): void {
  guest.setWindowOpenHandler(() => ({ action: 'deny' }));
  guest.on('will-navigate', (event, url) => {
    if (!sameScheme(url)) event.preventDefault();
  });
  guest.on('will-frame-navigate', (details) => {
    if (!sameScheme(details.url)) details.preventDefault();
  });
  guest.on('will-redirect', (event, url) => {
    if (!sameScheme(url)) event.preventDefault();
  });
  guest.setWebRTCIPHandlingPolicy('disable_non_proxied_udp');
  guest.on('render-process-gone', () => notify({ webContentsId: guest.id, reason: 'crashed' }));
  guest.on('before-input-event', (event, input) => {
    if (input.type === 'keyDown' && input.key === 'Escape') {
      event.preventDefault();
      notify({ webContentsId: guest.id, reason: 'escape' });
    }
  });
}

/** Lock the visualization partition down and answer its scheme. */
export async function installVisualizationSession(session: Session, router: VisualizationRouter): Promise<void> {
  session.protocol.handle(VISUALIZATION_SCHEME, async (request) => {
    const served = await router.serve(request.url).catch(() => notFound());
    return new Response(served.body.length ? new Uint8Array(served.body) : null, {
      status: served.status,
      headers: served.headers.map(([name, value]) => [name, value] as [string, string]),
    });
  });
  session.webRequest.onBeforeRequest((details, callback) => {
    const allowed = sameScheme(details.url) || details.url.startsWith('data:') || details.url.startsWith('blob:');
    callback({ cancel: !allowed });
  });
  session.setPermissionRequestHandler((_contents, _permission, callback) => callback(false));
  session.setPermissionCheckHandler(() => false);
  session.setDevicePermissionHandler(() => false);
  session.on('will-download', (event) => event.preventDefault());
  // Anything that escapes the request filter (WebRTC, prefetch) meets a dead
  // proxy. `<-loopback>` withdraws Chromium's implicit loopback bypass, so the
  // bridge-server's localhost port is unreachable too.
  await session.setProxy({
    mode: 'fixed_servers',
    proxyRules: 'http=127.0.0.1:9;https=127.0.0.1:9;ftp=127.0.0.1:9;socks=127.0.0.1:9',
    proxyBypassRules: '<-loopback>',
  });
}
