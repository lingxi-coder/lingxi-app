/**
 * Private-page host for the Electron desktop.
 *
 * The `lingxi-page://page/<session>/<id>/<revision>` scheme is answered by
 * the bridge of that session. The guest partition is in memory, has no
 * preload, and cannot reach the network. This is not the visualization guest.
 */

import type { Event, Session, WebContents, WebPreferences } from 'electron';
import type { PrivatePageServeDto } from '@lingxi/bridge-client';

import {
  PRIVATE_PAGE_PARTITION,
  PRIVATE_PAGE_SCHEME,
  parsePrivatePageUrl,
} from '../shared/privatePage.js';

export interface PrivatePageBackend {
  pageServe(id: string, revision: number): Promise<PrivatePageServeDto>;
}

export interface PrivatePageBackends {
  get(sessionId: string): PrivatePageBackend | undefined;
}

export interface ServedPrivatePage {
  readonly status: number;
  readonly headers: ReadonlyArray<readonly [string, string]>;
  readonly body: Buffer;
}

function notFound(): ServedPrivatePage {
  return {
    status: 404,
    headers: [['Content-Type', 'text/plain; charset=utf-8'], ['Cache-Control', 'no-store']],
    body: Buffer.from('not found'),
  };
}

export class PrivatePageRouter {
  constructor(private readonly backends: PrivatePageBackends) {}

  async serve(rawUrl: string): Promise<ServedPrivatePage> {
    const target = parsePrivatePageUrl(rawUrl);
    if (!target) return notFound();
    const backend = this.backends.get(target.sessionId);
    if (!backend) return notFound();
    try {
      return decode(await backend.pageServe(target.id, target.revision));
    } catch {
      return notFound();
    }
  }
}

function decode(served: PrivatePageServeDto): ServedPrivatePage {
  return {
    status: served.status,
    headers: served.headers,
    body: Buffer.from(served.body_base64, 'base64'),
  };
}

function pageGuestPreferences(): WebPreferences {
  return {
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
  };
}

/**
 * Allow a private-page webview, or refuse one that claims this partition.
 * Returns false when the request is not a private page, so the visualization
 * guard can decide.
 */
export function guardPrivatePageWebview(
  event: Pick<Event, 'preventDefault'>,
  webPreferences: WebPreferences & { preloadURL?: string },
  params: Record<string, string>,
): boolean {
  const src = params['src'] ?? '';
  const claimsPage = params['partition'] === PRIVATE_PAGE_PARTITION
    || src.startsWith(`${PRIVATE_PAGE_SCHEME}:`);
  if (!claimsPage) return false;
  const allowed = parsePrivatePageUrl(src) !== null
    && params['partition'] === PRIVATE_PAGE_PARTITION
    && !params['allowpopups']
    && !params['nodeintegration']
    && !params['nodeintegrationinsubframes']
    && !params['plugins']
    && !params['disablewebsecurity'];
  if (!allowed) {
    event.preventDefault();
    return true;
  }
  delete webPreferences.preloadURL;
  for (const key of Object.keys(webPreferences)) delete (webPreferences as Record<string, unknown>)[key];
  Object.assign(webPreferences, pageGuestPreferences(), {
    partition: PRIVATE_PAGE_PARTITION,
    disablePopups: true,
  });
  return true;
}

function samePage(raw: string): boolean {
  return parsePrivatePageUrl(raw) !== null;
}

/** Keep a private-page guest inside its own scheme. */
export function hardenPrivatePageGuest(guest: WebContents): void {
  guest.setWindowOpenHandler(() => ({ action: 'deny' }));
  guest.on('will-navigate', (event, url) => {
    if (!samePage(url)) event.preventDefault();
  });
  guest.on('will-frame-navigate', (details) => {
    if (!samePage(details.url)) details.preventDefault();
  });
  guest.on('will-redirect', (event, url) => {
    if (!samePage(url)) event.preventDefault();
  });
  guest.setWebRTCIPHandlingPolicy('disable_non_proxied_udp');
}

/** Lock the private-page partition down and answer its scheme. */
export async function installPrivatePageSession(session: Session, router: PrivatePageRouter): Promise<void> {
  session.protocol.handle(PRIVATE_PAGE_SCHEME, async (request) => {
    const served = await router.serve(request.url).catch(() => notFound());
    return new Response(served.body.length ? new Uint8Array(served.body) : null, {
      status: served.status,
      headers: served.headers.map(([name, value]) => [name, value] as [string, string]),
    });
  });
  session.webRequest.onBeforeRequest((details, callback) => {
    const allowed = samePage(details.url) || details.url.startsWith('data:') || details.url.startsWith('blob:');
    callback({ cancel: !allowed });
  });
  session.setPermissionRequestHandler((_contents, _permission, callback) => callback(false));
  session.setPermissionCheckHandler(() => false);
  session.setDevicePermissionHandler(() => false);
  session.on('will-download', (event) => event.preventDefault());
  await session.setProxy({
    mode: 'fixed_servers',
    proxyRules: 'http=127.0.0.1:9;https=127.0.0.1:9;ftp=127.0.0.1:9;socks=127.0.0.1:9',
    proxyBypassRules: '<-loopback>',
  });
}
