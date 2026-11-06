import { localAppMarketUrl } from '../../shared/productConfig.js';
import { MarketLinkInstaller } from './marketLinkInstaller.js';
import { nativeTr } from '../../shared/nativeLocale.js';
import { BrowserWindow, dialog, session, net, shell, clipboard, Notification, ipcMain, systemPreferences, type IpcMainInvokeEvent } from 'electron';
import { createServer, type Server } from 'node:http';
import { randomBytes, randomUUID } from 'node:crypto';
import { promises as fs, constants as fsConstants } from 'node:fs';
import { dirname, isAbsolute, join, relative, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { DesktopAppManifest, DesktopLocalApp, DesktopAuthoringRef, LocalAppCapability, LocalAppAgentQuestion, LocalAppEditRequest } from '../../shared/localApps.js';
import { isDesktopAuthoringRef } from '../../shared/localApps.js';
import type { AskQuestionDto } from '@lingxi/bridge-client';
import { LOCAL_APP_AGENT_MAX_FILE_BYTES, LOCAL_APP_AGENT_MAX_FILES, LOCAL_APP_AGENT_MAX_TOTAL_FILE_BYTES, type LocalAppAgentCatalog, type LocalAppAgentSettings } from '../../shared/localAppAgent.js';
import type { LocalAppAgentBroker, LocalAppAgentOwner, LocalAppAgentAuthorization } from './agentBroker.js';
import { canonicalJson, LocalAppAgentError, sameAgentGrant, validateAgentSettings } from './agentPolicy.js';
import { CH_LOCAL_APPS, CH_LOCAL_APPS_CHANGED, LOCAL_APP_SCHEME } from '../../shared/localApps.js';
import { CH_LOCAL_APP_NATIVE } from '../../shared/localAppNative.js';
import { appId, atomicJson, containedFile } from './store.js';
import { CoreLocalAppStore, type DesktopAuthoringGrant, type DesktopAuthoringSourceProof } from './coreStore.js';
import { AuthoringSourceIntents, type CopyIntent } from './authoringSourceIntents.js';
import { AuthoringSessionIntents, validateNewAuthoringIntent, type AuthoringSessionIntent, type AuthoringSessionSource } from './authoringSessionIntents.js';
import type { LocalAppNewSessionIntent } from '../../shared/localApps.js';
import type { PublicationIntent } from './publicationIntents.js';
import type { LocalAppCoreTransport } from './coreClient.js';
import { LocalAppMarketClient, type MarketPublication } from './marketClient.js';
import { LocalAppServiceBroker } from './serviceBroker.js';
import { LOCAL_APP_CREDENTIALS_HEADER, serviceCookieHeader, storeServiceCookies } from './serviceCookies.js';
import { LocalAppRuntime, type RuntimeOptions } from './runtime.js';
import { relativePath } from './manifest.js';
import { isAppUrl, permissionCapabilities } from './permissions.js';
import { LOCAL_APP_SDK, REMOTE_LOCAL_APP_SDK } from '../../preload/localAppSdk.js';
import { LocalAppRemoteGateway, type RemoteOwner, type RemoteRun } from './remoteGateway.js';
import type { LocalAppRtc } from './remoteRtc.js';
import { LOCAL_APP_RTC_BROWSER_SOURCE } from './remoteRtcBrowser.js';
import { LocalAppMcpManager } from './mcp.js';
import { LocalAppMcpGateway } from './mcpGateway.js';

function withAbort<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const abort = () => reject(signal.reason);
    signal.addEventListener('abort', abort, { once: true });
    operation.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort));
    if (signal.aborted) abort();
  });
}

function appEntryUrl(id: string, entry: DesktopAppManifest['entry']): string {
  const path = entry.type === 'server' && entry.path ? entry.path.replace(/^\/+/, '').split('/').map(encodeURIComponent).join('/') : '';
  return new URL(`${LOCAL_APP_SCHEME}://${id}/${path}`).href;
}

function htmlTagEnd(html: string, start: number): number | undefined {
  let quote = '';
  for (let index = start; index < html.length; index++) {
    const character = html[index]!;
    if (quote) {
      if (character === quote) quote = '';
    } else if (character === '"' || character === "'") quote = character;
    else if (character === '>') return index + 1;
  }
}

interface LocalAppHtmlTag {
  nameStart: number;
  nameEnd: number;
  end: number;
  closing: boolean;
  declaration: boolean;
}

function isHtmlWhitespace(value: string | undefined): boolean {
  return value === ' ' || value === '\t' || value === '\n' || value === '\f' || value === '\r';
}

function trimHtmlWhitespace(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isHtmlWhitespace(value[start])) start++;
  while (end > start && isHtmlWhitespace(value[end - 1])) end--;
  return value.slice(start, end);
}

function nextHtmlTag(html: string, from: number): LocalAppHtmlTag | undefined {
  let cursor = from;
  while (cursor < html.length) {
    const start = html.indexOf('<', cursor);
    if (start < 0) return;
    if (html.startsWith('<!--', start)) {
      const commentEnd = html.indexOf('-->', start + 4);
      if (commentEnd < 0) return;
      cursor = commentEnd + 3;
      continue;
    }
    const declaration = html[start + 1] === '!' || html[start + 1] === '?';
    const closing = !declaration && html[start + 1] === '/';
    const nameStart = start + 1 + Number(declaration || closing);
    if (!declaration && !/[a-z]/i.test(html[nameStart] ?? '')) {
      cursor = start + 1;
      continue;
    }
    let nameEnd = nameStart;
    while (/[a-z0-9:-]/i.test(html[nameEnd] ?? '')) nameEnd++;
    const boundary = html[nameEnd];
    if (!declaration && boundary !== undefined && boundary !== '>' && boundary !== '/' && !isHtmlWhitespace(boundary)) {
      // Consume malformed tag-like text as a unit, so a pseudo-tag in one of
      // its quoted attributes cannot be mistaken for an insertion point.
      cursor = htmlTagEnd(html, start) ?? start + 1;
      continue;
    }
    const end = htmlTagEnd(html, start);
    if (end === undefined) return;
    return { nameStart, nameEnd, end, closing, declaration };
  }
}

function matchingTagBoundary(value: string | undefined): boolean {
  return isHtmlWhitespace(value) || value === '/' || value === '>';
}

function asciiLowercase(value: string): string {
  return value.replace(/[A-Z]/g, character => String.fromCharCode(character.charCodeAt(0) + 32));
}

function skipScriptElement(html: string, lower: string, afterOpeningTag: number): number {
  let state: 'data' | 'escaped' | 'double-escaped' = 'data';
  let cursor = afterOpeningTag;
  while (cursor < html.length) {
    if (state !== 'data' && lower.startsWith('-->', cursor)) {
      state = 'data';
      cursor += 3;
      continue;
    }
    if (state === 'data' && lower.startsWith('<!--', cursor)) {
      state = 'escaped';
      cursor += 4;
      continue;
    }
    if (state === 'double-escaped' && lower.startsWith('</script', cursor)
      && matchingTagBoundary(html[cursor + 8])) {
      state = 'escaped';
      cursor += 8;
      continue;
    }
    if ((state === 'data' || state === 'escaped') && lower.startsWith('</script', cursor)
      && matchingTagBoundary(html[cursor + 8])) {
      return htmlTagEnd(html, cursor) ?? html.length;
    }
    if (state === 'escaped' && lower.startsWith('<script', cursor)
      && matchingTagBoundary(html[cursor + 7])) {
      state = 'double-escaped';
      cursor += 7;
      continue;
    }
    cursor++;
  }
  return html.length;
}

function skipRawTextElement(html: string, lower: string, afterOpeningTag: number, name: string): number {
  if (name === 'plaintext') return html.length;
  if (name === 'script') return skipScriptElement(html, lower, afterOpeningTag);
  const closingTag = new RegExp(`</${name}(?=[\\t\\n\\f\\r />])`, 'ig');
  closingTag.lastIndex = afterOpeningTag;
  const match = closingTag.exec(html);
  return match ? htmlTagEnd(html, match.index) ?? html.length : html.length;
}

function nextRawTextCursor(html: string, lower: string, tag: LocalAppHtmlTag): number {
  if (!tag.closing && !tag.declaration) {
    const name = html.slice(tag.nameStart, tag.nameEnd).toLowerCase();
    if (['script', 'style', 'textarea', 'title', 'noscript', 'iframe', 'xmp', 'noembed', 'noframes', 'plaintext'].includes(name)) {
      return skipRawTextElement(html, lower, tag.end, name);
    }
  }
  return tag.end;
}

function skipTemplateElement(html: string, lower: string, afterOpeningTag: number): number {
  let depth = 1;
  let cursor = afterOpeningTag;
  while (true) {
    const tag = nextHtmlTag(html, cursor);
    if (!tag) return html.length;
    if (!tag.declaration && html.slice(tag.nameStart, tag.nameEnd).toLowerCase() === 'template') {
      if (tag.closing) {
        depth--;
        if (depth === 0) return tag.end;
      } else {
        depth++;
      }
    }
    // This cursor sees nested templates to balance their end tags, while the
    // ordinary scanner skips the complete inert template subtree at once.
    cursor = nextRawTextCursor(html, lower, tag);
  }
}

function nextHtmlTagCursor(html: string, lower: string, tag: LocalAppHtmlTag): number {
  if (!tag.closing && !tag.declaration && html.slice(tag.nameStart, tag.nameEnd).toLowerCase() === 'template') {
    return skipTemplateElement(html, lower, tag.end);
  }
  return nextRawTextCursor(html, lower, tag);
}

interface LocalAppScriptAttributes {
  src?: string;
  type?: string;
  async: boolean;
  defer: boolean;
  noModule: boolean;
  integrity: boolean;
  href?: string;
}

function scriptAttributes(html: string, tag: LocalAppHtmlTag): LocalAppScriptAttributes {
  const attributes = html.slice(tag.nameEnd, tag.end - 1);
  let cursor = 0;
  const seen = new Set<string>();
  const result: LocalAppScriptAttributes = { async: false, defer: false, noModule: false, integrity: false };
  while (cursor < attributes.length) {
    while (isHtmlWhitespace(attributes[cursor]) || attributes[cursor] === '/') cursor++;
    const nameStart = cursor;
    while (cursor < attributes.length && !isHtmlWhitespace(attributes[cursor]) && !/[=/>]/.test(attributes[cursor]!)) cursor++;
    if (cursor === nameStart) { cursor++; continue; }
    const name = attributes.slice(nameStart, cursor).toLowerCase();
    while (isHtmlWhitespace(attributes[cursor])) cursor++;
    let value = '';
    if (attributes[cursor] === '=') {
      cursor++;
      while (isHtmlWhitespace(attributes[cursor])) cursor++;
      const quote = attributes[cursor] === '"' || attributes[cursor] === "'" ? attributes[cursor++] : undefined;
      const valueStart = cursor;
      if (quote) {
        while (cursor < attributes.length && attributes[cursor] !== quote) cursor++;
      } else {
        while (cursor < attributes.length && !isHtmlWhitespace(attributes[cursor]) && attributes[cursor] !== '>') cursor++;
      }
      value = attributes.slice(valueStart, cursor);
      if (quote && cursor < attributes.length) cursor++;
    }
    if (seen.has(name)) continue;
    seen.add(name);
    if (name === 'src') result.src = value;
    else if (name === 'type') result.type = value;
    else if (name === 'async') result.async = true;
    else if (name === 'defer') result.defer = true;
    else if (name === 'nomodule') result.noModule = true;
    else if (name === 'integrity') result.integrity = true;
    else if (name === 'href') result.href = value;
  }
  return result;
}

const JAVASCRIPT_MIME_TYPES = new Set([
  'application/ecmascript', 'application/javascript', 'application/x-ecmascript',
  'application/x-javascript', 'text/ecmascript', 'text/javascript', 'text/javascript1.0',
  'text/javascript1.1', 'text/javascript1.2', 'text/javascript1.3', 'text/javascript1.4',
  'text/javascript1.5', 'text/jscript', 'text/livescript', 'text/x-ecmascript', 'text/x-javascript',
]);

function isClassicScript(attributes: LocalAppScriptAttributes): boolean {
  const type = trimHtmlWhitespace(attributes.type ?? '').toLowerCase();
  if (!type) return true;
  return JAVASCRIPT_MIME_TYPES.has(trimHtmlWhitespace(type.split(';', 1)[0]!));
}

function mayRunBeforeLaterHostScript(attributes: LocalAppScriptAttributes): boolean {
  const type = trimHtmlWhitespace(attributes.type ?? '').toLowerCase();
  if (type === 'module') return attributes.async;
  if (!isClassicScript(attributes)) return false;
  if (attributes.src === undefined) return true;
  return attributes.async || attributes.noModule || !attributes.defer;
}

function scriptTagHasLocalAppSource(
  attributes: LocalAppScriptAttributes,
  appId: string,
  path: string,
  documentBase: string,
): boolean {
  if (attributes.src === undefined) return false;
  try {
    const source = new URL(attributes.src, documentBase);
    return source.protocol === `${LOCAL_APP_SCHEME}:` && source.hostname === appId && source.pathname === path;
  } catch { /* An invalid URL does not satisfy the host resource binding. */ }
  return false;
}

function isLocalAppHostScript(
  attributes: LocalAppScriptAttributes,
  appId: string,
  documentBase: string,
): boolean {
  return scriptTagHasLocalAppSource(attributes, appId, '/__lingxi_local_app_sdk.js', documentBase);
}

function reliableHostScript(attributes: LocalAppScriptAttributes): boolean {
  return attributes.src !== undefined && isClassicScript(attributes)
    && !attributes.async && !attributes.defer && !attributes.noModule && !attributes.integrity;
}

function htmlHasLocalAppScript(html: string, lower: string, appId: string, path: string, documentPath: string): boolean {
  let cursor = 0;
  let earlierBlockingApplicationScript = false;
  let documentBase = `${LOCAL_APP_SCHEME}://${appId}${documentPath}`;
  let baseHrefSeen = false;
  while (true) {
    const tag = nextHtmlTag(html, cursor);
    if (!tag) return false;
    const name = html.slice(tag.nameStart, tag.nameEnd).toLowerCase();
    if (!tag.closing && !tag.declaration && name === 'base') {
      const attributes = scriptAttributes(html, tag);
      if (!baseHrefSeen && attributes.href !== undefined) {
        baseHrefSeen = true;
        try {
          const candidate = new URL(attributes.href, documentBase);
          if (candidate.protocol !== 'data:' && candidate.protocol !== 'javascript:') {
            documentBase = candidate.href;
          }
        } catch { /* Invalid and forbidden base URLs leave the document URL in effect. */ }
      }
    } else if (!tag.closing && !tag.declaration && name === 'script') {
      const attributes = scriptAttributes(html, tag);
      const hostScript = scriptTagHasLocalAppSource(attributes, appId, path, documentBase);
      if (hostScript && reliableHostScript(attributes) && !earlierBlockingApplicationScript) return true;
      if (!isLocalAppHostScript(attributes, appId, documentBase)
        && mayRunBeforeLaterHostScript(attributes)) {
        earlierBlockingApplicationScript = true;
      }
    }
    cursor = nextHtmlTagCursor(html, lower, tag);
  }
}

function injectLocalAppScripts(html: string, appId: string, documentPath: string): string {
  const lower = asciiLowercase(html);
  const localAppAlreadyLoaded = htmlHasLocalAppScript(html, lower, appId, '/__lingxi_local_app_sdk.js', documentPath);
  const scripts = localAppAlreadyLoaded ? '' : '<script src="/__lingxi_local_app_sdk.js"></script>';
  if (!scripts) return html;
  const doctype = findLeadingDoctypeEnd(html, lower);
  const insertionIndex = doctype ?? (html.charCodeAt(0) === 0xfeff ? 1 : 0);
  return html.slice(0, insertionIndex) + scripts + html.slice(insertionIndex);
}

function isLeadingHtmlPreamble(value: string): boolean {
  let cursor = value.charCodeAt(0) === 0xfeff ? 1 : 0;
  while (cursor < value.length) {
    if (isHtmlWhitespace(value[cursor])) {
      cursor++;
      continue;
    }
    if (value.startsWith('<!--', cursor)) {
      const end = value.indexOf('-->', cursor + 4);
      if (end < 0) return false;
      cursor = end + 3;
      continue;
    }
    if (value.startsWith('<?', cursor) || value.startsWith('<!', cursor)) {
      const end = value.indexOf('>', cursor + 2);
      if (end < 0) return false;
      cursor = end + 1;
      continue;
    }
    return false;
  }
  return true;
}

function findLeadingDoctypeEnd(html: string, lower: string): number | undefined {
  let cursor = 0;
  while (true) {
    const tag = nextHtmlTag(html, cursor);
    if (!tag) return;
    if (tag.declaration && html.slice(tag.nameStart, tag.nameEnd).toLowerCase() === 'doctype') {
      const tagStart = tag.nameStart - 2;
      return isLeadingHtmlPreamble(html.slice(0, tagStart)) ? tag.end : undefined;
    }
    if (!tag.declaration) return;
    cursor = nextHtmlTagCursor(html, lower, tag);
  }
}

const FILE_TYPES: Record<string, { extensions: string[]; mediaType: string }> = {
  '.txt': { extensions: ['txt'], mediaType: 'text/plain' }, '.md': { extensions: ['md'], mediaType: 'text/markdown' },
  '.csv': { extensions: ['csv'], mediaType: 'text/csv' }, '.json': { extensions: ['json'], mediaType: 'application/json' },
  '.pdf': { extensions: ['pdf'], mediaType: 'application/pdf' }, '.png': { extensions: ['png'], mediaType: 'image/png' },
  '.jpg': { extensions: ['jpg', 'jpeg'], mediaType: 'image/jpeg' }, '.jpeg': { extensions: ['jpg', 'jpeg'], mediaType: 'image/jpeg' },
  '.gif': { extensions: ['gif'], mediaType: 'image/gif' }, '.webp': { extensions: ['webp'], mediaType: 'image/webp' },
};
const ACCEPT_WILDCARDS: Record<string, string[]> = { 'image/*': ['png', 'jpg', 'jpeg', 'gif', 'webp'], 'text/*': ['txt', 'md', 'csv'] };
function fileAccept(value: unknown): { tokens: string[]; extensions: string[] } {
  if (value === undefined) return { tokens: [], extensions: [] };
  const values = Array.isArray(value) ? value : [value];
  if (values.length > 32 || values.some(item => typeof item !== 'string' || item.length > 512)) throw new Error('Invalid file accept option');
  const tokens = values.flatMap(item => (item as string).split(',')).map(item => item.trim().toLowerCase()).filter(Boolean);
  const extensions = new Set<string>();
  for (const token of tokens) {
    if (/^\.[a-z0-9]{1,16}$/.test(token)) extensions.add(token.slice(1));
    else if (ACCEPT_WILDCARDS[token]) ACCEPT_WILDCARDS[token]!.forEach(extension => extensions.add(extension));
    else if (/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/.test(token)) {
      for (const [extension, type] of Object.entries(FILE_TYPES)) if (type.mediaType === token) type.extensions.forEach(ext => extensions.add(ext));
    } else throw new Error('Invalid file accept option');
  }
  return { tokens, extensions: [...extensions] };
}
function acceptedFile(path: string, mediaType: string, tokens: string[]): boolean {
  if (!tokens.length) return true;
  const extension = path.slice(path.lastIndexOf('.')).toLowerCase();
  return tokens.some(token => token === extension || token === mediaType || (token.endsWith('/*') && mediaType.startsWith(`${token.slice(0, -1)}`)));
}
function mediaTypeForPath(path: string): string { return FILE_TYPES[path.slice(path.lastIndexOf('.')).toLowerCase()]?.mediaType ?? 'application/octet-stream'; }

export interface LocalAppHostOptions extends RuntimeOptions {
  configHome: string;
  appProjectsRoot: string;
  onRemoteAccessEnded?: () => void;
  core: LocalAppCoreTransport;
  accountToken?: () => Promise<string>;
  accountId?: () => string | undefined;
  /** Trusted, host-owned current mode; captured once when an IPC request arrives. */
  currentConversationMode: () => 'chat' | 'code' | undefined;
  resolveAuthoringBinding?: (appId: string | undefined, session: { projectPath: string; sessionId: string }, source?: AuthoringSessionSource) => Promise<Omit<DesktopAuthoringRef, 'workspaceId' | 'appId'>>;
  onRunEnded?: (appId: string, runId: string, reason: string) => Promise<void> | void;
  preload: string;
  mainWindow: () => BrowserWindow | undefined;
  edit: (request: LocalAppEditRequest) => Promise<void>;
  agentCatalog?: () => Promise<LocalAppAgentCatalog>;
  onMcpChanged?: () => void;
}
interface SourceSessionRef { projectPath: string; sessionId: string }
type LocalAppRequestOrigin = { kind: 'native'; mode: 'chat' | 'code' | undefined; authoringOrder?: number; recoverExplicitBinding?: boolean; authoringSource?: AuthoringSessionSource } | { kind: 'tool' };
const DEVELOPMENT_ACTIONS = new Set(['prepareAuthoring', 'bindAuthoring', 'publish', 'read', 'edit', 'restoreSource', 'remix', 'marketPublish', 'resumePublication', 'resumeSourceCopy', 'resumeAuthoringSession']);
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
function absoluteCanonicalPath(value: unknown): value is string {
  return typeof value === 'string' && isAbsolute(value) && !value.includes('\0')
    && !value.split(/[\\/]/).some(part => part === '.' || part === '..');
}
function completeDesktopAuthoring(value: unknown): value is DesktopAuthoringRef {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const authoring = value as DesktopAuthoringRef;
  if (!isDesktopAuthoringRef(authoring)) return false;
  return UUID_PATTERN.test(authoring.workspaceId) && UUID_PATTERN.test(authoring.authoringSessionId)
    && absoluteCanonicalPath(authoring.sourceDirectory)
    && authoring.projectRef?.kind === 'desktopPath' && absoluteCanonicalPath(authoring.projectRef.path)
    && (authoring.appId === undefined || /^a[a-f0-9]{32}$/.test(authoring.appId));
}
function sourceSessionRef(value: unknown): SourceSessionRef | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return;
  const row = value as Record<string, unknown>;
  if (!absoluteCanonicalPath(row.projectPath) || typeof row.sessionId !== 'string' || !UUID_PATTERN.test(row.sessionId)) return;
  return { projectPath: row.projectPath, sessionId: row.sessionId };
}

export class LocalAppHost {
  readonly store: CoreLocalAppStore;
  readonly services = new LocalAppServiceBroker();
  readonly market: LocalAppMarketClient;
  private readonly marketLinkInstaller: MarketLinkInstaller;
  readonly runtime: LocalAppRuntime;
  private server?: Server;
  private remoteGateway?: LocalAppRemoteGateway;
  private remoteRtc?: LocalAppRtc;
  private readonly remoteAgentSessions = new WeakSet<AbortSignal>();
  private remoteHtmlBytes = 0;
  private remoteSettingsTail: Promise<void> = Promise.resolve();
  private readonly remoteDisabled = new Set<string>();
  private remoteEpoch = 0;
  private readonly remoteAppEpochs = new Map<string, number>();
  private readonly remoteOpening = new Map<string, Set<AbortController>>();
  private readonly remoteOpeningOwners = new WeakMap<AbortController, string>();
  private readonly remoteManagedRuns = new Map<string, string>();
  private readonly remoteCleaning = new Map<string, Promise<void>>();
  private readonly token = randomBytes(32).toString('hex');
  private readonly windows = new Map<string, BrowserWindow>();
  private readonly windowRuns = new WeakMap<BrowserWindow, string>();
  private readonly opening = new Map<string, { promise: Promise<void>; controller: AbortController; intent: string; waiters: number }>();
  private readonly waitingOpenings = new Map<string, Set<AbortController>>();
  private readonly openingEpochs = new Map<string, number>();
  private readonly configured = new Set<string>();
  private readonly trustedWindows = new Map<number, string>();
  private changeTimer?: ReturnType<typeof setTimeout>;
  private shuttingDown = false;
  private unavailable?: Error;
  private agentBroker?: LocalAppAgentBroker;
  private unsubscribeAgentSettlement?: () => void;
  private readonly blockedAgentApps = new Set<string>();
  private readonly agentEpochs = new Map<string, number>();
  private readonly backgroundAgentOwners = new Map<string, { revision: number; invocationId: string }>();
  private readonly navigationEpoch = new Map<number, number>();
  private readonly agentQuestions = new Map<string, { view: LocalAppAgentQuestion; appId: string; resolve: (answers: Record<string, string> | null) => void }>();
  private authoringSourceTail: Promise<void> = Promise.resolve();
  private readonly authoringSourceIntents: AuthoringSourceIntents;
  private readonly authoringSessionIntents: AuthoringSessionIntents;
  private readonly authoringIntents = new Map<string, symbol>();
  private readonly authoringVersions = new Map<string, number>();
  private readonly authoringOrders = new Map<string, number>();
  private readonly authoringVerifications = new Map<string, Set<Promise<void>>>();
  private authoringSequence = 0;
  private readonly mcp: LocalAppMcpManager;
  private readonly mcpGateway: LocalAppMcpGateway;
  constructor(private readonly options: LocalAppHostOptions) {
    this.store = new CoreLocalAppStore(join(options.configHome, 'local-apps', 'v2'), options.core);
    this.authoringSourceIntents = new AuthoringSourceIntents(this.store.root);
    this.authoringSessionIntents = new AuthoringSessionIntents(this.store.root);
    this.market = new LocalAppMarketClient(this.store, options.accountToken ?? (() => Promise.reject(new Error('Sign in before publishing to the market'))), undefined, undefined, options.accountId);
    this.marketLinkInstaller = new MarketLinkInstaller({
      list: () => this.store.list(),
      preview: id => this.market.request(`apps/${encodeURIComponent(id)}`),
      confirm: async app => {
        const options = { type: 'question' as const, title: 'Local App Market', message: `Install ${app.name.slice(0, 160)}?`,
          detail: `${app.summary.slice(0, 1000)}\n\nDownload and add this application to your local library. Running it will use the normal application permission flow.`,
          buttons: ['Cancel', 'Install'], defaultId: 1, cancelId: 0 };
        const parent = this.options.mainWindow();
        const result = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
        return result.response === 1;
      },
      install: id => this.market.install(id),
      installed: () => this.changed(),
    });
    this.mcp = new LocalAppMcpManager({ changed: () => { this.changed(); options.onMcpChanged?.(); } });
    this.mcpGateway = new LocalAppMcpGateway(options.configHome, this.mcp);
    this.runtime = new LocalAppRuntime(this.store, () => this.changed(), { ...options, mcp: this.mcp,
      onRunEnded: (appId, runId, reason) => this.runEnded(appId, runId, reason) });
    options.core.onDisconnected?.(error => {
      this.unavailable = error;
      void Promise.allSettled([this.mcp.dispose(), this.mcpGateway.dispose()]);
      void this.agentBroker?.dispose().catch(() => undefined);
      this.remoteRevokeAll();
      this.launchGrants.clear();
      for (const opening of this.opening.values()) opening.controller.abort();
      for (const waiting of this.waitingOpenings.values()) for (const controller of waiting) controller.abort();
      for (const window of this.windows.values()) window.destroy();
      void this.services.dispose();
      // No automatic bridge reconnect: native cleanup must finish before a new
      // exclusive owner can recover historical run metadata.
      void this.runtime.dispose().catch(() => undefined);
      this.changed();
    });
  }
  private async withAuthoringSourceLock<T>(operation: () => Promise<T>): Promise<T> {
    const previous = this.authoringSourceTail;
    let release!: () => void;
    this.authoringSourceTail = new Promise<void>(resolve => { release = resolve; });
    await previous;
    try { return await operation(); } finally { release(); }
  }
  /** Called only by the Host after its Code-mode admission, before runtime boot. */
  beginAuthoringSession(input: LocalAppNewSessionIntent): {
    allocate(ref: SourceSessionRef): Promise<void>; recordSource(source: AuthoringSessionSource): Promise<void>; finish(ref: SourceSessionRef): Promise<void>;
  } {
    const intent = validateNewAuthoringIntent(input);
    const order = intent.appId && intent.setCurrent ? ++this.authoringSequence : undefined;
    let saved: AuthoringSessionIntent | undefined;
    return {
      allocate: async ref => { saved = await this.authoringSessionIntents.create(ref, intent); this.changed(); },
      recordSource: async source => {
        if (!saved) throw new Error('The Local App session owner is unavailable');
        saved = await this.authoringSessionIntents.recordSource(saved.requestId, source);
      },
      finish: async ref => {
        if (!saved || saved.projectPath !== ref.projectPath || saved.sessionId !== ref.sessionId) throw new Error('The Local App session owner changed');
        await this.finishAuthoringSession(saved, order);
      },
    };
  }
  async getAuthoringSession(requestId: string): Promise<AuthoringSessionIntent> {
    const saved = await this.authoringSessionIntents.get(requestId);
    if (!saved || saved.state === 'completed') throw new Error('The pending Local App session is unavailable');
    return saved;
  }
  async recordAuthoringSessionSource(requestId: string, source: AuthoringSessionSource): Promise<AuthoringSessionIntent> {
    return this.authoringSessionIntents.recordSource(requestId, source);
  }
  async finishAuthoringSession(saved: AuthoringSessionIntent, order?: number): Promise<void> {
    if (!saved.source) throw new Error('The Local App session source has not been confirmed');
    const session = { projectPath: saved.projectPath, sessionId: saved.sessionId };
    const action = saved.appId
      ? { action: 'bindAuthoring', id: saved.appId, session, setCurrent: order !== undefined && saved.setCurrent,
        ...(saved.sourceRequestId ? { sourceRequestId: saved.sourceRequestId } : {}) }
      : { action: 'prepareAuthoring', session };
    await this.request(action, undefined, { kind: 'native', mode: 'code', authoringOrder: order,
      recoverExplicitBinding: order === undefined && saved.setCurrent, authoringSource: saved.source });
    await this.authoringSessionIntents.ready(saved.requestId);
    this.changed();
  }
  async completeAuthoringSession(ref: SourceSessionRef): Promise<void> {
    await this.authoringSessionIntents.complete(ref); this.changed();
  }
  private beginAuthoringIntent(id: string, order = ++this.authoringSequence): { current: () => boolean; finish: () => void } {
    const token = Symbol('authoring intent');
    if (order < (this.authoringOrders.get(id) ?? 0)) return { current: () => false, finish: () => {} };
    this.authoringOrders.set(id, order);
    this.authoringVersions.set(id, (this.authoringVersions.get(id) ?? 0) + 1);
    this.authoringIntents.set(id, token);
    return { current: () => this.authoringIntents.get(id) === token,
      finish: () => { if (this.authoringIntents.get(id) === token) this.authoringIntents.delete(id); } };
  }
  private trackAuthoringVerification(id: string): () => void {
    let release!: () => void;
    const pending = new Promise<void>(resolve => { release = resolve; });
    const entries = this.authoringVerifications.get(id) ?? new Set<Promise<void>>();
    entries.add(pending); this.authoringVerifications.set(id, entries);
    return () => { entries.delete(pending); if (!entries.size) this.authoringVerifications.delete(id); release(); };
  }
  private checkAuthoringSource(proof: DesktopAuthoringSourceProof, sourceDirectory: string, requestId?: string, targetAppId?: string): void {
    if (!proof || !UUID_PATTERN.test(proof.sourceId) || proof.sourceDirectory !== sourceDirectory || proof.sourcePath !== sourceDirectory
      || (requestId !== undefined && (proof.requestId !== requestId || proof.targetAppId !== targetAppId))) {
      throw new Error('The Local App source proof does not match the selected Project');
    }
  }
  private checkAuthoringGrant(grant: DesktopAuthoringGrant, sourceId: string, session: SourceSessionRef): void {
    if (!grant || grant.sourceId !== sourceId || grant.sessionMode !== 'code' || !completeDesktopAuthoring(grant.authoring)
      || grant.authoring.sourceDirectory !== session.projectPath || grant.authoring.projectRef.path !== session.projectPath
      || grant.authoring.authoringSessionId !== session.sessionId) {
      throw new Error('The Local App authoring receipt does not match its source session');
    }
  }
  private async existingAuthoringGrant(sourceDirectory: string, session: SourceSessionRef): Promise<DesktopAuthoringGrant | undefined> {
    try { return await this.store.getAuthoringGrant(sourceDirectory, { kind: 'desktopPath', path: session.projectPath }, session.sessionId); }
    catch (error) {
      if ((error as { code?: unknown })?.code === 'notFound') return;
      throw error;
    }
  }
  private async completeSourceCopy(saved: CopyIntent): Promise<CopyIntent> {
    if (saved.state !== 'pending') return saved;
    const request = saved.purpose === 'restoreSource'
      ? { op: 'copyAuthoringSource', requestId: saved.requestId, appId: saved.appId, revision: saved.revision,
        sourcePath: saved.sourcePath, projectRef: { kind: 'desktopPath', path: saved.sourcePath } }
      : { op: 'remixAuthoringSource', requestId: saved.requestId, appId: saved.appId, revision: saved.revision, sourcePath: saved.sourcePath };
    const result = await this.store.core.request<{ requestId: string; sourceId: string; sourceDirectory: string; sourcePath: string;
      targetAppId?: string; record?: { appId: string; latestRevision: number }; release?: { appId: string; revision: number } }>(request, { waitForCompletion: true });
    const targetAppId = saved.purpose === 'restoreSource' ? result.targetAppId : result.record?.appId;
    const canonicalSource = await fs.realpath(saved.sourcePath);
    if (result.requestId !== saved.requestId || result.sourceDirectory !== canonicalSource || result.sourcePath !== canonicalSource
      || canonicalSource !== saved.sourcePath || !UUID_PATTERN.test(result.sourceId)
      || typeof targetAppId !== 'string' || !/^a[a-f0-9]{32}$/.test(targetAppId)
      || (saved.purpose === 'restoreSource' && targetAppId !== saved.appId)
      || (saved.purpose === 'remix' && (targetAppId === saved.appId || result.release?.appId !== targetAppId
        || !Number.isSafeInteger(result.release.revision) || result.release.revision < 1
        || result.record?.latestRevision !== result.release.revision))) {
      throw new Error('The Local App source copy did not match its saved intent');
    }
    return this.authoringSourceIntents.recordCopy(saved.requestId, result.sourceId, targetAppId);
  }
  private async resolveNativeAuthoring(sessionRef: SourceSessionRef, source?: AuthoringSessionSource): Promise<Omit<DesktopAuthoringRef, 'workspaceId' | 'appId'>> {
    if (!this.options.resolveAuthoringBinding) throw new Error('Desktop cannot verify the selected app Code session');
    const resolved = await this.options.resolveAuthoringBinding(undefined, sessionRef, source);
    const [sourceDirectory, projectPath, projectRefPath] = await Promise.all([
      fs.realpath(resolved.sourceDirectory), fs.realpath(sessionRef.projectPath), fs.realpath(resolved.projectRef.path),
    ]);
    if (resolved.authoringSessionId !== sessionRef.sessionId || resolved.projectRef.kind !== 'desktopPath'
      || sourceDirectory !== projectPath || projectRefPath !== projectPath) {
      throw new Error('Create a dedicated Local App Project and Code session before publishing');
    }
    return { sourceDirectory, projectRef: { kind: 'desktopPath', path: projectPath }, authoringSessionId: sessionRef.sessionId };
  }
  private async prepareNativeAuthoring(sessionRef: SourceSessionRef, source?: AuthoringSessionSource): Promise<DesktopAuthoringRef> {
    const resolved = await this.resolveNativeAuthoring(sessionRef, source);
    return this.withAuthoringSourceLock(async () => {
      const requestId = await this.authoringSourceIntents.sourceRequestId({ purpose: 'create', sourceDirectory: resolved.sourceDirectory,
        projectPath: resolved.projectRef.path, sessionId: resolved.authoringSessionId });
      const proof = await this.store.ensureAuthoringSource(resolved.sourceDirectory, resolved.projectRef, requestId);
      this.checkAuthoringSource(proof, resolved.sourceDirectory, requestId);
      const session = { projectPath: resolved.projectRef.path, sessionId: resolved.authoringSessionId };
      const previous = await this.existingAuthoringGrant(resolved.sourceDirectory, session);
      if (previous) this.checkAuthoringGrant(previous, proof.sourceId, session);
      if (previous?.ownerAppId) throw new Error(`This Local App source already belongs to ${previous.ownerAppId}; continue developing that application instead`);
      const authoring: DesktopAuthoringRef = { ...resolved, workspaceId: previous?.authoring.workspaceId ?? randomUUID() };
      const saved = await this.store.registerAuthoring(proof.sourceId, requestId, authoring, true);
      this.checkAuthoringGrant({ sourceId: proof.sourceId, authoring: saved, sessionMode: saved.sessionMode }, proof.sourceId, session);
      if (saved.workspaceId !== authoring.workspaceId) throw new Error('The Local App authoring receipt changed during registration');
      return saved;
    });
  }
  private async verifySourcePath(input: Record<string, unknown>, sourceDirectory: string): Promise<void> {
    if (input.source_path === undefined) return;
    if (typeof input.source_path !== 'string') throw new Error('Invalid source path from the Local App tool');
    const sourcePath = await fs.realpath(input.source_path);
    const withinProject = relative(sourceDirectory, sourcePath);
    if (withinProject === '..' || withinProject.startsWith(`..${sep}`) || isAbsolute(withinProject)) {
      throw new Error('The staged app source must remain inside its native authoring Project');
    }
  }
  private async publishAuthoring(input: Record<string, unknown>, appIdValue: string | undefined,
    resolved: Omit<DesktopAuthoringRef, 'workspaceId' | 'appId'>, requestId: string): Promise<DesktopAuthoringGrant | undefined> {
    const app = appIdValue === undefined ? undefined : await this.store.get(appIdValue);
    const proof = await this.store.findAuthoringSource(resolved.sourceDirectory);
    if (!proof) {
      if (app?.authoring) throw new Error('Modify this application from its registered source Code session');
      let managedRoot: string | undefined;
      try { managedRoot = await fs.realpath(this.options.appProjectsRoot); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
      if (managedRoot) {
        const child = relative(managedRoot, resolved.sourceDirectory);
        if (child !== '..' && !child.startsWith(`..${sep}`) && !isAbsolute(child)) {
          throw new Error('The managed Local App source has no verified authoring receipt');
        }
      }
      return;
    }
    this.checkAuthoringSource(proof, resolved.sourceDirectory);
    await this.verifySourcePath(input, resolved.sourceDirectory);
    const session = { projectPath: resolved.projectRef.path, sessionId: resolved.authoringSessionId };
    const grant = await this.store.getAuthoringGrant(resolved.sourceDirectory, resolved.projectRef, session.sessionId);
    this.checkAuthoringGrant(grant, proof.sourceId, session);
    const prior = appIdValue === undefined && grant.ownerAppId !== undefined
      ? await this.store.publicationIntent(requestId) : undefined;
    const sameFirstPublication = prior?.appId === undefined && prior?.native?.sourceId === proof.sourceId
      && prior.sourceDirectory === resolved.sourceDirectory && prior.authoringSessionId === session.sessionId;
    if ((appIdValue === undefined && grant.ownerAppId !== undefined && !sameFirstPublication)
      || (appIdValue !== undefined && grant.ownerAppId !== appIdValue)) {
      throw new Error('The Local App authoring receipt belongs to another application');
    }
    return grant;
  }
  /** Capture the app's current native source owner before selecting its session. */
  async captureAuthoringSession(appIdValue: unknown, requested: SourceSessionRef, expectedVersion: number): Promise<{
    sourceDirectory: string; commit<T>(operation: () => T): Promise<T>;
  }> {
    const id = appId(appIdValue);
    const version = this.authoringVersions.get(id) ?? 0;
    if (!Number.isSafeInteger(expectedVersion) || expectedVersion !== version) throw new Error('The Local App authoring owner changed');
    const read = async (): Promise<{ sourceDirectory: string; sourceId: string; workspaceId: string }> => {
      if (this.unavailable || this.shuttingDown) throw new Error('The Local App host is unavailable');
      const row = await this.store.get(id);
      const authoring = row.authoring;
      if (!completeDesktopAuthoring(authoring) || authoring.appId !== id
        || authoring.authoringSessionId !== requested.sessionId || authoring.projectRef.path !== requested.projectPath
        || authoring.sourceDirectory !== authoring.projectRef.path) {
        throw new Error('The selected Local App authoring owner changed');
      }
      const proof = await this.store.findAuthoringSource(authoring.sourceDirectory);
      if (!proof) throw new Error('The Local App source has no verified authoring receipt');
      this.checkAuthoringSource(proof, authoring.sourceDirectory);
      const grant = await this.store.getAuthoringGrant(authoring.sourceDirectory, authoring.projectRef, authoring.authoringSessionId);
      this.checkAuthoringGrant(grant, proof.sourceId, requested);
      if (grant.ownerAppId !== id || grant.authoring.workspaceId !== authoring.workspaceId) {
        throw new Error('The Local App authoring receipt no longer matches its current owner');
      }
      return { sourceDirectory: authoring.sourceDirectory, sourceId: proof.sourceId, workspaceId: authoring.workspaceId };
    };
    const captured = await this.runtime.exclusive(id, async () => {
      const result = await read();
      if ((this.authoringVersions.get(id) ?? 0) !== version) throw new Error('The Local App authoring owner changed');
      return result;
    });
    return { sourceDirectory: captured.sourceDirectory, commit: async <T>(operation: () => T): Promise<T> =>
      this.runtime.exclusive(id, async () => {
        for (;;) {
          const pending = [...(this.authoringVerifications.get(id) ?? [])];
          if (pending.length) { await Promise.all(pending); continue; }
          const current = await read();
          if (this.authoringVerifications.get(id)?.size) continue;
          if ((this.authoringVersions.get(id) ?? 0) !== version || current.sourceDirectory !== captured.sourceDirectory
            || current.sourceId !== captured.sourceId || current.workspaceId !== captured.workspaceId) {
            throw new Error('The Local App authoring source changed');
          }
          return operation();
        }
      }) };
  }
  private async runEnded(appId: string, runId: string, reason: string): Promise<void> {
    this.remoteGateway?.revokeApp(appId, runId);
    this.services.revoke(appId, runId);
    const retained = reason === 'stopped' && this.backgroundAgentOwners.get(appId)?.invocationId === runId && !this.blockedAgentApps.has(appId);
    if (this.runtime.invocationId(appId) === runId && !retained) await this.cancelAgentApp(appId, 'Application run ended');
    await this.options.onRunEnded?.(appId, runId, reason);
  }
  setAgentBroker(broker: LocalAppAgentBroker): void {
    this.unsubscribeAgentSettlement?.();
    this.agentBroker = broker;
    this.unsubscribeAgentSettlement = broker.onSettled?.(owner => {
      if (this.remoteManagedRuns.get(owner.appId) === owner.invocationId) {
        void this.remoteAccessEnded(owner.appId, owner.invocationId).catch(() => undefined);
      }
    });
  }
  async authorizeAgent(owner: LocalAppAgentOwner): Promise<LocalAppAgentAuthorization> {
    const revision = this.runtime.revision(owner.appId);
    if (!revision) throw new Error('Application Agent access is no longer active');
    const row = await this.store.getRevision(owner.appId, revision);
    const settings = row.agent;
    const backgroundOwner = this.backgroundAgentOwners.get(owner.appId);
    const continuingBackground = settings?.background && backgroundOwner?.revision === revision && backgroundOwner.invocationId === owner.invocationId;
    if (this.shuttingDown || this.blockedAgentApps.has(owner.appId) || !settings?.enabled || !row.grants.includes('agent')
      || !this.launchGrants.get(owner.appId)?.includes('agent') || (this.runtime.phase(owner.appId) !== 'running' && !continuingBackground)
      || row.releaseDigest !== owner.releaseDigest || this.runtime.owner(owner.appId)?.releaseDigest !== owner.releaseDigest
      || this.runtime.invocationId(owner.appId) !== owner.invocationId) throw new Error('Application Agent access is no longer active');
    if (owner.remoteSessionId && !this.remoteGateway?.hasSession(owner.appId, owner.invocationId, owner.remoteSessionId)) throw new Error('Application Agent remote session is no longer active');
    const window = this.windows.get(owner.appId);
    if ((!window || window.isDestroyed()) && !settings.background && !this.remoteGateway?.hasRun(owner.appId, owner.invocationId)) throw new Error('Application Agent background access is not granted');
    if (window && owner.webContentsId !== undefined && window.webContents.id !== owner.webContentsId && !continuingBackground) throw new Error('Application Agent window changed');
    const requestedTools = this.runtime.manifest(owner.appId)?.agent?.tools ?? [];
    const manifestSkills = this.runtime.manifest(owner.appId)?.agent?.skills ?? [];
    const requestedSkills = manifestSkills.map(skill => skill.id);
    if ((settings.skills ?? []).some(skill => !requestedSkills.includes(skill))) throw new Error('Application Agent Skill access changed; review its permissions in Apps');
    const sourceRoot = this.store.revisionPath(owner.appId, revision);
    const release = await this.store.core.request<{ digest: string; files: Array<{ path: string; sha256: string; bytes: number; executable: boolean }> }>({
      op: 'getRelease', appId: owner.appId, revision,
    });
    if (release.digest !== owner.releaseDigest) throw new Error('Application Agent release changed; review its permissions in Apps');
    const skillPrefixes = manifestSkills.map(skill => skill.path.slice(0, -'SKILL.md'.length));
    const releaseFiles = release.files.filter(file => skillPrefixes.some(prefix => file.path.startsWith(prefix)));
    const catalog = await this.options.agentCatalog?.();
    if (!catalog) throw new Error('Application Agent host is unavailable');
    const model = catalog.models.find(m => m.providerId === settings.model.providerId && m.modelId === settings.model.modelId && m.canSelect);
    if (!model) throw new Error('Select an available Agent model in Apps');
    if (settings.tools.some(grant => !requestedTools.includes(grant.id) || !catalog.tools.some(current => sameAgentGrant(grant, current)))) throw new Error('Application Agent tool access changed; review its permissions in Apps');
    if (this.blockedAgentApps.has(owner.appId) || this.runtime.invocationId(owner.appId) !== owner.invocationId
      || this.runtime.owner(owner.appId)?.releaseDigest !== owner.releaseDigest
      || canonicalJson((await this.store.get(owner.appId)).agent) !== canonicalJson(settings)
      || (owner.remoteSessionId && !this.remoteGateway?.hasSession(owner.appId, owner.invocationId, owner.remoteSessionId))) throw new Error('Application Agent access changed');
    return { settings: { ...structuredClone(settings), skills: [...(settings.skills ?? [])] }, requestedTools: [...requestedTools], requestedSkills: [...requestedSkills],
      releaseRevision: revision, releaseSourceDirectory: sourceRoot, skillSpecs: structuredClone(manifestSkills), releaseFiles, model };
  }
  async revalidateAgent(owner: LocalAppAgentOwner, authorization: LocalAppAgentAuthorization): Promise<boolean> {
    try {
      const current = await this.authorizeAgent(owner);
      return current.settings.model.providerId === authorization.settings.model.providerId && current.settings.model.modelId === authorization.settings.model.modelId
        && current.model.supportsAttachments === authorization.model.supportsAttachments && current.model.supportsVision === authorization.model.supportsVision
        && current.model.supportsStructuredOutput === authorization.model.supportsStructuredOutput
        && (current.settings.background || !authorization.settings.background) && current.settings.maxTurns >= authorization.settings.maxTurns && current.settings.timeoutMs >= authorization.settings.timeoutMs
        && authorization.settings.tools.every(grant => current.settings.tools.some(tool => sameAgentGrant(grant, tool)))
        && authorization.settings.skills.every(skill => current.settings.skills.includes(skill))
        && current.releaseRevision === authorization.releaseRevision && current.releaseSourceDirectory === authorization.releaseSourceDirectory
        && canonicalJson(current.skillSpecs) === canonicalJson(authorization.skillSpecs)
        && canonicalJson(current.releaseFiles) === canonicalJson(authorization.releaseFiles);
    } catch { return false; }
  }
  async promptAgentPermission(input: { appId: string; runId: string; toolName: string; input: unknown }, signal: AbortSignal): Promise<boolean> {
    signal.throwIfAborted();
    const row = await this.store.get(input.appId);
    const options = { type: 'question' as const, signal, title: `${row.name} · Agent`, message: `Allow ${row.name}'s Agent to use ${input.toolName}?`,
      detail: JSON.stringify(input.input, null, 2).slice(0, 16000), buttons: ['Deny', 'Allow once'], defaultId: 0, cancelId: 0 };
    const parent = this.options.mainWindow();
    const result = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
    return !signal.aborted && !this.blockedAgentApps.has(input.appId) && result.response === 1;
  }
  async promptAgentQuestion(input: { appId: string; runId: string; requestId: number; questions: AskQuestionDto[] }, signal: AbortSignal): Promise<{ answers: Array<{ question: string; answer: string }> } | null> {
    signal.throwIfAborted();
    const row = await this.store.get(input.appId), id = randomUUID();
    const parent = this.options.mainWindow();
    if (!parent || parent.isDestroyed()) throw new Error('Open LingXi to answer the application Agent');
    return new Promise(resolve => {
      const finish = (answers: Record<string, string> | null) => {
        signal.removeEventListener('abort', abort); this.agentQuestions.delete(id); this.changed();
        resolve(answers ? { answers: input.questions.map(question => ({ question: question.question, answer: answers[question.question]! })) } : null);
      };
      const abort = () => finish(null);
      this.agentQuestions.set(id, { appId: input.appId, view: { id, appName: row.name, request: { request_id: input.requestId, questions: input.questions } }, resolve: finish });
      signal.addEventListener('abort', abort, { once: true });
      if (signal.aborted) { abort(); return; }
      parent.show(); parent.focus(); this.changed();
    });
  }
  private async cancelAgentApp(id: string, reason: string): Promise<number> {
    const epoch = (this.agentEpochs.get(id) ?? 0) + 1;
    this.agentEpochs.set(id, epoch);
    this.blockedAgentApps.add(id);
    this.backgroundAgentOwners.delete(id);
    for (const pending of this.agentQuestions.values()) if (pending.appId === id) pending.resolve(null);
    await this.agentBroker?.cancelApp(id, reason);
    return epoch;
  }
  registerMainWindow(window: BrowserWindow, url: string): void {
    const id = window.webContents.id;
    this.trustedWindows.set(id, url);
    window.once('closed', () => this.trustedWindows.delete(id));
  }
  private assertMain(event: IpcMainInvokeEvent): void {
    const expected = this.trustedWindows.get(event.sender.id);
    if (!expected || event.senderFrame !== event.sender.mainFrame || !event.senderFrame || event.senderFrame.url.split('#')[0] !== expected.split('#')[0]) throw new Error('Unauthorized application library caller');
  }
  changed(): void {
    if (this.changeTimer || this.shuttingDown) return;
    this.changeTimer = setTimeout(() => {
      this.changeTimer = undefined;
      for (const window of BrowserWindow.getAllWindows()) if (this.trustedWindows.has(window.webContents.id)) window.webContents.send(CH_LOCAL_APPS_CHANGED);
    }, 80);
  }
  async initialize(): Promise<void> {
    await fs.mkdir(this.store.importsRoot, { recursive: true, mode: 0o700 });
    await this.mcpGateway.initialize();
    ipcMain.handle(CH_LOCAL_APPS, (event, input) => {
      this.assertMain(event);
      const mode = this.options.currentConversationMode();
      return this.request(input, undefined, { kind: 'native', mode });
    });
    ipcMain.handle(CH_LOCAL_APP_NATIVE, (event, input) => this.native(event, input));
    // Model tools use this authenticated local-only endpoint. No account or
    // provider credentials are passed to it or to application children.
    this.server = createServer(async (request, response) => {
      if (request.method !== 'POST' || request.url !== '/local-app' || request.headers.authorization !== `Bearer ${this.token}` || request.headers.origin) {
        response.writeHead(403).end(); return;
      }
      const controller = new AbortController();
      response.once('close', () => { if (!response.writableFinished) controller.abort(); });
      try {
        const chunks: Buffer[] = []; let size = 0;
        for await (const chunk of request) { size += chunk.length; if (size > 64 * 1024 * 1024) throw new Error('Local App request is too large'); chunks.push(chunk); }
        const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
        if (!['publish', 'list', 'read', 'start', 'stop', 'status', 'logs', 'agent_catalog'].includes(input.action)) throw new Error('Unsupported LocalApp tool action');
        controller.signal.throwIfAborted();
        const result = await this.request(input, controller.signal, { kind: 'tool' });
        response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }).end(JSON.stringify(result ?? { ok: true }));
      } catch (error) { response.writeHead(400, { 'Content-Type': 'application/json' }).end(JSON.stringify({ error: error instanceof Error ? error.message : String(error) })); }
    });
    await new Promise<void>((resolve, reject) => { this.server!.once('error', reject); this.server!.listen(0, '127.0.0.1', resolve); });
    const address = this.server.address();
    if (!address || typeof address === 'string') throw new Error('Local App host could not listen');
    await atomicJson(join(this.options.configHome, 'desktop-local-app-host.json'), { port: address.port, token: this.token, pid: process.pid });
  }
  async list(): Promise<DesktopLocalApp[]> { return (await this.store.list()).map(row => this.runtime.status(row)); }
  private get remoteSettingsPath(): string { return join(this.store.root, 'remote-access.v1.json'); }
  async remoteSettings(): Promise<Record<string, boolean>> {
    try {
      const data = JSON.parse(await fs.readFile(this.remoteSettingsPath, 'utf8')) as { version?: number; enabled?: unknown };
      if (data.version !== 1 || !Array.isArray(data.enabled) || data.enabled.some(id => typeof id !== 'string' || !/^a[a-f0-9]{32}$/.test(id))) throw new Error('Invalid remote access settings');
      return Object.fromEntries(data.enabled.map(id => [id, true]));
    } catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return {}; throw error; }
  }
  async setRemoteAccess(id: string, enabled: boolean): Promise<void> {
    appId(id); if (typeof enabled !== 'boolean') throw new Error('Invalid remote access setting');
    const previous = this.remoteSettingsTail;
    let release!: () => void; this.remoteSettingsTail = new Promise<void>(resolve => { release = resolve; });
    await previous;
    try {
      if (!enabled) { this.invalidateRemoteOpenings(id); this.remoteDisabled.add(id); this.remoteGateway?.revokeApp(id); }
      await this.store.get(id);
      const settings = await this.remoteSettings();
      const installed = new Set((await this.store.list()).map(row => row.id));
      for (const shared of Object.keys(settings)) if (!installed.has(shared)) delete settings[shared];
      if (enabled && !settings[id] && Object.keys(settings).length >= 256) throw new Error('At most 256 applications can be shared. Disable sharing for another application first.');
      if (enabled) settings[id] = true; else delete settings[id];
      await atomicJson(this.remoteSettingsPath, { version: 1, enabled: Object.keys(settings).sort() });
      if (enabled) this.remoteDisabled.delete(id);
      this.changed();
    } finally { release(); }
  }
  async remoteList(): Promise<DesktopLocalApp[]> {
    const enabled = await this.remoteSettings();
    const rows = (await this.list()).filter(row => enabled[row.id]);
    return Promise.all(rows.map(async row => this.runtime.status(await this.store.get(row.id))));
  }
  async remoteCurrent(run: RemoteRun): Promise<boolean> {
    try {
      if (this.shuttingDown || this.unavailable || this.remoteDisabled.has(run.appId) || !(await this.remoteSettings())[run.appId] || this.remoteDisabled.has(run.appId)) return false;
      const row = await this.store.get(run.appId);
      return row.revision === run.revision && this.runtime.revision(run.appId) === run.revision
        && this.runtime.phase(run.appId) === 'running' && this.runtime.invocationId(run.appId) === run.runId;
    } catch { return false; }
  }
  remoteConfiguration(): { configured: boolean; originTemplate?: string; port?: number; error?: string } {
    const originTemplate = process.env.LINGXI_LOCAL_APP_REMOTE_ORIGIN, rawPort = process.env.LINGXI_LOCAL_APP_GATEWAY_PORT;
    try {
      if (!originTemplate || !rawPort || !/^\d+$/.test(rawPort)) throw new Error('Set LINGXI_LOCAL_APP_REMOTE_ORIGIN to an HTTPS origin using {appId}-{sessionId} as its first DNS label, and set LINGXI_LOCAL_APP_GATEWAY_PORT');
      const port = Number(rawPort);
      new LocalAppRemoteGateway(originTemplate, port, { current: async () => false, resource: async () => new Response(null), invoke: async () => undefined, invokeGuard: () => () => false, socket: async () => undefined });
      return { configured: true, originTemplate, port };
    } catch (error) { return { configured: false, error: error instanceof Error ? error.message : 'Invalid gateway configuration' }; }
  }
  setRemoteRtc(rtc: LocalAppRtc): void { this.remoteRtc?.dispose(); this.remoteRtc = rtc; }
  private remoteInvokeGuard(run: RemoteRun, method: string): () => boolean {
    const agentEpoch = this.agentEpochs.get(run.appId) ?? 0;
    return () => !this.shuttingDown && !this.unavailable && this.runtime.revision(run.appId) === run.revision
      && this.runtime.phase(run.appId) === 'running' && this.runtime.invocationId(run.appId) === run.runId
      && (!method.startsWith('agent.') || (!this.blockedAgentApps.has(run.appId) && (this.agentEpochs.get(run.appId) ?? 0) === agentEpoch));
  }
  private gateway(): LocalAppRemoteGateway {
    if (!this.remoteGateway) {
      const origin = process.env.LINGXI_LOCAL_APP_REMOTE_ORIGIN, rawPort = process.env.LINGXI_LOCAL_APP_GATEWAY_PORT;
      if (!origin || !rawPort || !/^\d+$/.test(rawPort)) throw new Error('Remote Local App gateway is not configured');
      this.remoteGateway = new LocalAppRemoteGateway(origin, Number(rawPort), {
        current: run => this.remoteCurrent(run), resource: (run, request) => this.remoteResource(run, request),
        invoke: (run, method, args, signal, sessionId) => this.remoteInvoke(run, method, args, signal, sessionId),
        invokeGuard: (run, method) => this.remoteInvokeGuard(run, method),
        socket: async (run, name) => {
          if (!await this.remoteCurrent(run)) return;
          const entry = this.runtime.manifest(run.appId)?.entry;
          return this.runtime.service(run.appId, name || (entry?.type === 'server' ? entry.process : ''))?.port;
        },
        rtc: (run, request, sessionId, signal) => this.remoteRtc ? this.remoteRtc.rtc(run, request, sessionId, signal) : Promise.resolve(new Response('Not found', { status: 404 })),
        onAccessEnded: (id, runId) => { void this.remoteAccessEnded(id, runId); },
      });
    }
    return this.remoteGateway;
  }
  async remoteOpen(id: string, owner: RemoteOwner): Promise<{ app_id: string; revision: number; url: string; expires_at: number }> {
    const controller = new AbortController();
    const openings = this.remoteOpening.get(id) ?? new Set<AbortController>();
    this.remoteOpeningOwners.set(controller, JSON.stringify([owner.accountUid, owner.deviceId, owner.hostId]));
    openings.add(controller); this.remoteOpening.set(id, openings);
    // Allow first-launch installation and trusted permission prompts, but never
    // leave abandoned startup work running indefinitely after the client times out.
    const timer = setTimeout(() => controller.abort(new Error('Remote application launch timed out')), 900_000); timer.unref();
    const operation = this.remoteOpenOnce(id, owner, controller.signal);
    const settled = () => {
      // A shared start or its activation commit can settle after the caller's
      // deadline. Reconcile that late run even though the public call is over.
      if (!this.remoteOpening.has(id)) { const run = this.runtime.invocationId(id); if (run) void this.remoteAccessEnded(id, run); }
    };
    void operation.then(settled, settled);
    try { return await withAbort(operation, controller.signal); }
    finally {
      clearTimeout(timer); openings.delete(controller);
      if (!openings.size) { this.remoteOpening.delete(id); const run = this.runtime.invocationId(id); if (run) void this.remoteAccessEnded(id, run); }
    }
  }
  private async remoteOpenOnce(id: string, owner: RemoteOwner, signal: AbortSignal): Promise<{ app_id: string; revision: number; url: string; expires_at: number }> {
    signal.throwIfAborted();
    const epoch = this.remoteEpoch;
    const appEpoch = this.remoteAppEpochs.get(id) ?? 0;
    const changed = () => epoch !== this.remoteEpoch || appEpoch !== (this.remoteAppEpochs.get(id) ?? 0);
    appId(id); if (this.remoteDisabled.has(id) || !(await this.remoteSettings())[id] || this.remoteDisabled.has(id) || changed()) throw new Error('Remote access is not enabled for this application');
    const gateway = this.gateway();
    const cleaning = this.remoteCleaning.get(id);
    if (cleaning) await withAbort(cleaning, signal);
    const pending = this.opening.get(id);
    if (pending) await withAbort(pending.promise.catch(() => undefined), signal);
    signal.throwIfAborted();
    const row = await this.store.get(id);
    await this.store.assertTarget(row);
    await this.authorize(id, signal, row.revision);
    const agentEpoch = this.agentEpochs.get(id);
    if (changed()) throw new Error('Remote access changed');
    const backgroundOwner = this.backgroundAgentOwners.get(id);
    if (backgroundOwner && backgroundOwner.revision === row.revision && backgroundOwner.invocationId === this.runtime.invocationId(id)) {
      await this.runtime.resume(id, backgroundOwner.revision, backgroundOwner.invocationId, signal);
    } else {
      await this.runtime.start(id, false, signal, this.runtime.revision(id) !== undefined && this.runtime.revision(id) !== row.revision, row.revision);
    }
    const startedRun = this.runtime.invocationId(id); if (startedRun) this.remoteManagedRuns.set(id, startedRun);
    if (this.agentEpochs.get(id) === agentEpoch) this.blockedAgentApps.delete(id);
    signal.throwIfAborted();
    if (changed()) throw new Error('Remote access changed');
    this.launchGrants.set(id, (await this.store.getRevision(id, row.revision)).grants.filter(cap => this.runtime.manifest(id)?.capabilities?.includes(cap)));
    const revision = this.runtime.revision(id), runId = this.runtime.invocationId(id);
    if (!revision || !runId) throw new Error('Application run is unavailable');
    signal.throwIfAborted();
    if (changed()) throw new Error('Remote access changed');
    return gateway.open({ appId: id, revision, runId }, owner, signal);
  }
  private remoteAccessEnded(id: string, runId: string): Promise<void> {
    return this.queueRunCleanup(id, () => this.cleanRemoteRun(id, runId)).finally(() => {
      // A dedicated hidden RTC window counts as an Electron window too. Release
      // it before the app checks whether any foreground/background user remains.
      if (this.shuttingDown) return;
      if (!this.remoteOpening.size && !this.remoteGateway?.hasAccess()) this.remoteRtc?.releaseIdle();
      this.options.onRemoteAccessEnded?.();
    });
  }
  private queueRunCleanup(id: string, operation: () => Promise<void>): Promise<void> {
    const previous = this.remoteCleaning.get(id) ?? Promise.resolve();
    const cleanup = previous.catch(() => undefined).then(operation).finally(() => {
      if (this.remoteCleaning.get(id) === cleanup) this.remoteCleaning.delete(id);
    });
    this.remoteCleaning.set(id, cleanup); return cleanup;
  }
  private async cleanRemoteRun(id: string, runId: string): Promise<void> {
    try {
      const stillUnused = () => this.remoteManagedRuns.get(id) === runId && !this.shuttingDown && !this.remoteOpening.has(id) && !this.opening.has(id) && !this.windows.has(id)
        && this.runtime.invocationId(id) === runId && !this.remoteGateway?.hasRun(id, runId);
      if (!stillUnused()) return;
      const row = await this.store.get(id);
      if (!stillUnused() || row.background || (row.agent?.enabled && row.agent.background && this.agentBroker?.hasActiveApp(id))) return;
      await this.cancelAgentApp(id, 'Remote application session ended');
      if (stillUnused()) { await this.runtime.stop(id); if (this.remoteManagedRuns.get(id) === runId) this.remoteManagedRuns.delete(id); }
    } catch { /* Run may already be stopping or removed. */ }
  }
  private invalidateRemoteOpenings(id?: string): void {
    if (id === undefined) { this.remoteEpoch++; this.remoteAppEpochs.clear(); }
    else this.remoteAppEpochs.set(id, (this.remoteAppEpochs.get(id) ?? 0) + 1);
    const groups = id === undefined ? this.remoteOpening.values() : [this.remoteOpening.get(id) ?? []];
    for (const openings of groups) for (const controller of openings) controller.abort(new Error('Remote access changed'));
  }
  remoteRevokeOwner(owner: RemoteOwner): void {
    const key = JSON.stringify([owner.accountUid, owner.deviceId, owner.hostId]);
    for (const openings of this.remoteOpening.values()) for (const controller of openings) if (this.remoteOpeningOwners.get(controller) === key) controller.abort(new Error('Remote access changed'));
    this.remoteGateway?.revokeOwner(owner);
  }
  remoteRevokeTicket(url: string): void { this.remoteGateway?.revokeTicket(url); }
  remoteRevokeAll(): void { this.invalidateRemoteOpenings(); this.remoteGateway?.revokeAll(); }
  async remoteResource(run: RemoteRun, request: Request): Promise<Response> {
    if (!await this.remoteCurrent(run)) return new Response('Session expired', { status: 410 });
    const { appId: id } = run, url = new URL(request.url), manifest = this.runtime.manifest(id), workspace = this.runtime.workspace(id);
    if (!manifest || !workspace) return new Response('Application unavailable', { status: 410 });
    if (url.pathname === '/__lingxi_local_app_sdk.js') return new Response(REMOTE_LOCAL_APP_SDK + (this.remoteRtc ? `\n${LOCAL_APP_RTC_BROWSER_SOURCE}` : ''), { headers: { 'content-type': 'text/javascript; charset=utf-8' } });
    if (url.pathname.startsWith('/__lingxi_remote_')) return new Response('Not found', { status: 404 });
    if (manifest.entry.type === 'server' && url.pathname === '/' && manifest.entry.path && manifest.entry.path !== '/') {
      return new Response(null, { status: 307, headers: { location: manifest.entry.path + url.search } });
    }
    const route = /^\/__services\/([\w-]+)(\/.*)?$/.exec(url.pathname);
    let response: Response;
    let remoteService: { port: number; prefix: string } | undefined;
    if (manifest.entry.type === 'server' || route) {
      const name = route?.[1] ?? (manifest.entry.type === 'server' ? manifest.entry.process : '');
      const service = this.runtime.service(id, name);
      if (!service) return new Response('Backend unavailable', { status: 503 });
      const serviceHeaders = new Headers(request.headers);
      for (const key of [...serviceHeaders.keys()]) if (key === 'forwarded' || key === 'x-real-ip' || key.startsWith('x-forwarded-')) serviceHeaders.delete(key);
      serviceHeaders.set('x-forwarded-host', url.host);
      serviceHeaders.set('x-forwarded-proto', url.protocol.slice(0, -1));
      const serviceRequest = new Request(request, { headers: serviceHeaders });
      remoteService = { port: service.port, prefix: route ? `/__services/${name}` : '' };
      response = await this.services.fetch(id, run.runId, name, service.port, (route ? route[2] ?? '/' : url.pathname) + url.search, serviceRequest, request.headers.get(LOCAL_APP_CREDENTIALS_HEADER) === 'omit' ? '' : request.headers.get('cookie') ?? '', true);
    } else {
      const assetRoot = dirname(manifest.entry.path);
      const path = url.pathname === '/' ? manifest.entry.path : join(assetRoot, decodeURIComponent(url.pathname).replace(/^\/+/, '')).split(sep).join('/');
      let file: string;
      try { file = await containedFile(workspace, path); }
      catch { if (request.headers.get('accept')?.includes('text/html')) file = await containedFile(workspace, manifest.entry.path); else return new Response('Not found', { status: 404 }); }
      response = await net.fetch(pathToFileURL(file).toString());
    }
    const headers = new Headers(response.headers);
    const location = headers.get('location');
    if (remoteService?.prefix && location?.startsWith('/') && !location.startsWith('//')) {
      const target = new URL(location, url.origin);
      headers.set('location', `${url.origin}${remoteService.prefix}${target.pathname}${target.search}${target.hash}`);
    }
    if (remoteService && location && /^http:\/\/(?:127\.0\.0\.1|localhost):[0-9]+(?=[/?#]|$)/i.test(location)) {
      try {
        const target = new URL(location);
        if (target.protocol === 'http:' && !target.username && !target.password && ['127.0.0.1', 'localhost'].includes(target.hostname) && Number(target.port || 80) === remoteService.port) {
          headers.set('location', `${url.origin}${remoteService.prefix}${target.pathname}${target.search}${target.hash}`);
        }
      } catch { /* Preserve malformed or external redirects unchanged. */ }
    }
    let body: BodyInit | null = response.body;
    const injectedHtml = response.status !== 206 && headers.get('content-type')?.toLowerCase().includes('text/html');
    // Fetch only decodes recognized content codings. A remaining encoding must
    // never be interpreted as UTF-8 and advertised as a newly injected document.
    if (injectedHtml && ![204, 205, 304].includes(response.status)
      && headers.get('content-encoding')?.split(',').some(coding => coding.trim().toLowerCase() !== 'identity')) {
      await response.body?.cancel();
      return new Response('Unsupported application HTML encoding', { status: 502, headers: { 'content-type': 'text/plain; charset=utf-8' } });
    }
    if (injectedHtml) for (const name of ['etag', 'content-md5', 'digest', 'content-digest', 'repr-digest']) headers.delete(name);
    if (request.method === 'HEAD' || [204, 205, 304].includes(response.status)) {
      await response.body?.cancel(); body = null;
      // The normal HTML representation is injected, so the upstream length
      // is not the length a GET would return.
      if (request.method === 'HEAD' && injectedHtml) headers.delete('content-length');
    } else if (injectedHtml && response.body) {
      body = injectLocalAppScripts(await this.remoteHtml(response.body, request.signal), id, url.pathname); headers.delete('content-length'); headers.delete('content-encoding');
    }
    headers.set('content-security-policy', "default-src 'self' data: blob: https: http: ws: wss:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob: https: http:; style-src 'self' 'unsafe-inline' https: http:; worker-src 'self' blob:; object-src 'none'; frame-ancestors 'none'");
    return new Response(body, { status: response.status, headers });
  }
  private async remoteHtml(body: ReadableStream<Uint8Array>, signal: AbortSignal): Promise<string> {
    const reader = body.getReader(), chunks: Uint8Array[] = [];
    let size = 0;
    const abort = () => { void reader.cancel().catch(() => undefined); };
    signal.addEventListener('abort', abort, { once: true });
    try {
      for (;;) {
        signal.throwIfAborted();
        const next = await reader.read(); if (next.done) break;
        if (size + next.value.byteLength > 8 * 1024 * 1024 || this.remoteHtmlBytes + next.value.byteLength > 32 * 1024 * 1024) throw new Error('Application HTML exceeds response budget');
        size += next.value.byteLength; this.remoteHtmlBytes += next.value.byteLength; chunks.push(next.value);
      }
      signal.throwIfAborted();
      return Buffer.concat(chunks, size).toString('utf8');
    } finally {
      this.remoteHtmlBytes -= size; signal.removeEventListener('abort', abort); await reader.cancel().catch(() => undefined);
    }
  }
  private async remoteInvoke(run: RemoteRun, method: string, args: Record<string, unknown>, signal: AbortSignal, sessionId: string): Promise<unknown> {
    const invokeCurrent = this.remoteInvokeGuard(run, method);
    const current = async () => !signal.aborted && await this.remoteCurrent(run) && !signal.aborted && invokeCurrent();
    const guardedRead = async <T>(operation: Promise<T>): Promise<T> => {
      try {
        const value = await operation;
        if (!await current()) throw new Error('Application Agent access changed');
        return value;
      } catch (error) {
        if (!await current()) throw new Error('Application Agent access changed');
        throw error;
      }
    };
    if (signal.aborted || !await this.remoteCurrent(run) || signal.aborted) throw new Error('Application operation is no longer active');
    const row = await this.store.getRevision(run.appId, run.revision);
    if (method === 'info') return { id: run.appId, name: row.name, runId: run.runId, revision: run.revision, platform: 'browser' };
    if (method === 'capability.clipboard' && row.grants.includes('clipboard') && this.launchGrants.get(run.appId)?.includes('clipboard')) return;
    if (!method.startsWith('agent.') || !row.grants.includes('agent') || !this.launchGrants.get(run.appId)?.includes('agent') || this.blockedAgentApps.has(run.appId)) throw new Error('Application capability is unavailable');
    const active = this.runtime.owner(run.appId), broker = this.agentBroker;
    if (!broker || !active || active.releaseDigest !== row.releaseDigest) throw new Error('Application Agent host is unavailable');
    const owner: LocalAppAgentOwner = { appId: run.appId, releaseDigest: active.releaseDigest, invocationId: run.runId, remoteSessionId: sessionId };
    await this.authorizeAgent(owner);
    if (!await current() || signal.aborted || !invokeCurrent()) throw new Error('Application operation is no longer active');
    const cancelSession = async () => {
      try { await broker.cancelRemoteSession(owner); }
      finally {
        // The first access-ended callback may still see an active background
        // Agent while its executor is stopping. Reconcile again after it settles.
        await this.remoteAccessEnded(run.appId, run.runId);
      }
    };
    const start = async (operation: Promise<{runId: string}>) => {
      const result = await operation;
      if (!this.remoteAgentSessions.has(signal)) {
        this.remoteAgentSessions.add(signal);
        signal.addEventListener('abort', () => { void cancelSession().catch(() => undefined); }, { once: true });
      }
      if (signal.aborted) await cancelSession().catch(() => undefined);
      return result;
    };
    if (method === 'agent.tasks.start') return start(broker.run(owner, args.input, signal));
    if (method === 'agent.tasks.get') return guardedRead(broker.get(owner, String(args.runId)));
    if (method === 'agent.tasks.list') return guardedRead(broker.list(owner, args));
    if (method === 'agent.tasks.events') return guardedRead(broker.events(owner, String(args.runId), args.afterSequence as number | undefined));
    if (method === 'agent.tasks.cancel') return broker.cancel(owner, String(args.runId), signal);
    if (method === 'agent.chats.create' || method === 'agent.chats.creationStatus' || method === 'agent.chats.list') {
      // Match the local resource contract. An HTTP/transport failure stays
      // uncertain; only these host-owned results can resolve a pending create.
      try {
        const value = method === 'agent.chats.create' ? await broker.createChat(owner, args, signal)
          : method === 'agent.chats.creationStatus' ? await broker.chatCreationStatus(owner, args, signal) : await broker.listChats(owner, args);
        if (!await current()) throw new Error('Application Agent access changed');
        return { ok: true, value };
      } catch (error) {
        if (!await current()) return { ok: false, error: { code: 'host_unavailable', message: 'The Agent host is unavailable.' } };
        const codes = new Set(['invalid_chat', 'invalid_input', 'chat_create_conflict', 'chat_create_deleted', 'chat_storage_uncertain', 'agent_storage_unavailable']);
        const code = error instanceof LocalAppAgentError && codes.has(error.code) ? error.code : 'host_unavailable';
        return { ok: false, error: { code, message: code === 'host_unavailable' ? 'The Agent host is unavailable.' : (error as Error).message } };
      }
    }
    if (method === 'agent.chats.get') return guardedRead(broker.getChat(owner, String(args.chatId)));
    if (method === 'agent.chats.archive') return broker.archiveChat(owner, String(args.chatId), signal);
    if (method === 'agent.chats.delete') return broker.deleteChat(owner, String(args.chatId), signal);
    if (method === 'agent.chats.history') return guardedRead(broker.chatHistory(owner, String(args.chatId), args));
    if (method === 'agent.chats.send') return start(broker.sendChat(owner, String(args.chatId), args.input, signal));
    if (method === 'agent.chats.cancel') return broker.cancelChat(owner, String(args.chatId), signal);
    throw new Error('Unknown application Agent API');
  }
  private async editAuthoring(row: DesktopLocalApp, setCurrent = false, isCurrent: () => boolean = () => true): Promise<void> {
    const authoringVersion = this.authoringVersions.get(row.id) ?? 0;
    const sourceDirectory = row.authoring?.sourceDirectory;
    let canonicalSource: string | undefined;
    if (sourceDirectory) {
      try {
        const canonical = await fs.realpath(sourceDirectory);
        if ((await fs.stat(canonical)).isDirectory()) canonicalSource = canonical;
      } catch { /* native prepareAuthoring reports a source that has disappeared */ }
    }
    if (!canonicalSource) {
      if (isCurrent()) await this.options.edit({ id: row.id, name: row.name,
        ...(row.authoring ? { authoring: structuredClone(row.authoring) } : {}),
        authoringSourceMissing: Boolean(sourceDirectory), authoringAction: 'sourceMissing', authoringVersion });
      return;
    }
    const authoring = isDesktopAuthoringRef(row.authoring) ? row.authoring : undefined;
    const sessionBindings: Record<string, { workspaceId: string; sourceDirectory: string; projectRef: { kind: 'desktopPath'; path: string }; mode: 'code' }> = {};
    if (authoring && this.options.resolveAuthoringBinding) {
      let resolved: Omit<DesktopAuthoringRef, 'workspaceId' | 'appId'> | undefined;
      try {
        resolved = await this.options.resolveAuthoringBinding(row.id, {
          projectPath: authoring.projectRef.path, sessionId: authoring.authoringSessionId,
        });
      } catch { /* A missing session cannot be resumed. */ }
      if (resolved && resolved.sourceDirectory === canonicalSource && resolved.projectRef.path === canonicalSource
        && resolved.authoringSessionId === authoring.authoringSessionId) {
        const proof = await this.store.findAuthoringSource(canonicalSource);
        if (proof) {
          this.checkAuthoringSource(proof, canonicalSource);
          const session = { projectPath: canonicalSource, sessionId: authoring.authoringSessionId };
          const grant = await this.existingAuthoringGrant(canonicalSource, session);
          if (grant) this.checkAuthoringGrant(grant, proof.sourceId, session);
          if (grant?.ownerAppId === row.id && grant.authoring.workspaceId === authoring.workspaceId) {
            sessionBindings[authoring.authoringSessionId] = { workspaceId: authoring.workspaceId,
              sourceDirectory: authoring.sourceDirectory, projectRef: authoring.projectRef, mode: 'code' };
          }
        }
      }
    }
    const prepared = await this.store.core.request<{ action: 'sourceMissing' | 'rebindRequired' | 'resumeSession' | 'createSession'; sourceDirectory: string; authoring?: DesktopAuthoringRef; authoringSessionId?: string }>({
      op: 'prepareAuthoring', appId: row.id, sessionBindings, sourceExists: true,
    });
    if (isCurrent()) await this.options.edit({ id: row.id, name: row.name,
      authoring: prepared.authoring ?? { sourceDirectory: prepared.sourceDirectory || canonicalSource },
      authoringSourceMissing: false, authoringAction: prepared.action, authoringSetCurrent: setCurrent, authoringVersion });
  }
  async request(input: Record<string, unknown>, signal?: AbortSignal, origin: LocalAppRequestOrigin = {
    kind: 'native', mode: this.options.currentConversationMode(),
  }): Promise<unknown> {
    if (this.unavailable) throw this.unavailable;
    if (!input || typeof input !== 'object') throw new Error('Invalid Local App request');
    const action = String(input.action);
    if (origin.kind !== 'native' && (action === 'pendingAuthoringSessions' || action === 'resumeAuthoringSession' || action === 'authoringSessions')) {
      throw new Error('Local App session recovery is available only in Desktop');
    }
    if (origin.kind === 'native' && DEVELOPMENT_ACTIONS.has(action) && origin.mode !== 'code') {
      throw new Error(nativeTr('local_app_development_requires_code'));
    }
    // The model adapter injects the originating session from its trusted turn;
    // resolve it now, before waiting on app locks or writing any source state.
    let originatingAuthoring: Omit<DesktopAuthoringRef, 'workspaceId' | 'appId'> | undefined;
    let publicationRequestId: string | undefined;
    let priorPublication: PublicationIntent | undefined;
    let committedAuthoring: DesktopAuthoringGrant | null | undefined;
    if (action === 'publish' || (origin.kind === 'tool' && action === 'read')) {
      const session = sourceSessionRef(input.source_session);
      if (!session) throw new Error(nativeTr('local_app_development_requires_code'));
      if (origin.kind === 'tool' && (input.source_session as Record<string, unknown>).mode !== 'code') {
        throw new Error(nativeTr('local_app_development_requires_code'));
      }
      // Start the trusted owner lookup immediately, while the original
      // session may still be the displayed live draft. A durable pending
      // tool request may also use its captured Code owner after a later /cd.
      const resolution = this.resolveNativeAuthoring(session).then(value => ({ value }), error => ({ error }));
      if (action === 'publish') {
        const requestId = (input.source_session as Record<string, unknown>).requestId;
        if (typeof requestId !== 'string' || !UUID_PATTERN.test(requestId)) throw new Error('A durable Local App publication request is required');
        publicationRequestId = requestId;
        priorPublication = await this.store.publicationIntent(requestId);
        if (priorPublication) {
          if (priorPublication.sourceDirectory !== session.projectPath || priorPublication.authoringSessionId !== session.sessionId
            || priorPublication.appId !== input.id) throw new Error('The Local App publication request conflicts with its saved owner');
          const receipt = await this.store.core.request<{ sourceId?: string | null; publisher?: DesktopAuthoringRef | null; record: { appId: string } } | null>({ op: 'getPublication', requestId });
          if (receipt) {
            if (!/^a[a-f0-9]{32}$/.test(receipt.record?.appId ?? '')
              || (priorPublication.appId !== undefined && priorPublication.appId !== receipt.record.appId)) {
              throw new Error('The Local App publication receipt does not match its saved intent');
            }
            if (priorPublication.native) {
              const publisher = receipt.publisher;
              if (receipt.sourceId !== priorPublication.native.sourceId || !completeDesktopAuthoring(publisher)
                || publisher.sourceDirectory !== priorPublication.sourceDirectory || publisher.projectRef.path !== priorPublication.sourceDirectory
                || publisher.authoringSessionId !== priorPublication.authoringSessionId) {
                throw new Error('The Local App publication receipt does not match its saved owner');
              }
              committedAuthoring = { sourceId: receipt.sourceId, authoring: publisher, sessionMode: 'code', ownerAppId: receipt.record.appId };
            } else {
              if (receipt.sourceId != null || receipt.publisher != null) throw new Error('The Local App publication receipt has an unexpected authoring owner');
              committedAuthoring = null;
            }
          }
        }
      }
      const admittedToolReplay = origin.kind === 'tool' && priorPublication?.state === 'pending'
        && (input.source_session as Record<string, unknown>).mode === 'code';
      if (priorPublication && (committedAuthoring !== undefined || admittedToolReplay)) {
        originatingAuthoring = { sourceDirectory: priorPublication.sourceDirectory,
          projectRef: { kind: 'desktopPath', path: priorPublication.sourceDirectory }, authoringSessionId: priorPublication.authoringSessionId };
      } else {
        const result = await resolution;
        if ('error' in result) throw result.error;
        originatingAuthoring = result.value;
      }
    }
    if (action === 'remote-settings') return this.remoteSettings();
    if (action === 'remote-configuration') return this.remoteConfiguration();
    if (action === 'remote-access') { if (typeof input.enabled !== 'boolean') throw new Error('Invalid remote access setting'); await this.setRemoteAccess(appId(input.id), input.enabled); return; }
    if (action === 'marketBrowse') {
      const url = new URL(localAppMarketUrl);
      url.searchParams.set('platform', process.platform === 'darwin' ? 'macos' : process.platform === 'win32' ? 'windows' : 'linux');
      await shell.openExternal(url.href); return;
    }
    if (action === 'marketInstallLink') return this.marketLinkInstaller.install(input.remoteId);
    if (action === 'marketManage') { await shell.openExternal(new URL('/mine', this.market.base).href); return; }
    if (action === 'marketInstall') {
      if (typeof input.remoteId !== 'string') throw new Error('Choose a market application');
      const existing = input.id ? await this.store.get(appId(input.id)) : undefined;
      if (existing && existing.revision !== input.expectedRevision) throw new Error('Application changed; refresh before updating');
      const row = await this.market.install(input.remoteId, existing); if (existing) { this.invalidateRemoteOpenings(existing.id); this.remoteGateway?.revokeApp(existing.id); } this.changed(); return row;
    }
    if (action === 'list') {
      const apps = await this.list();
      return { action, apps: origin.kind === 'native' && origin.mode === 'code' ? apps
        : apps.map(({ authoring: _authoring, authoringSourceMissing: _missing, authoringAction: _action, ...app }) => app) };
    }
    if (action === 'pendingPublications') {
      const pending = await this.store.pendingPublications();
      return origin.kind === 'native' && origin.mode === 'code' ? pending
        : pending.map(({ sourceDirectory: _sourceDirectory, ...entry }) => entry);
    }
    if (action === 'pendingSourceCopies') {
      return (await this.authoringSourceIntents.pendingCopies()).map(({ requestId, appId, revision, purpose }) => ({ requestId, appId, revision, purpose }));
    }
    if (action === 'pendingAuthoringSessions') {
      return (await this.authoringSessionIntents.pending()).map(({ requestId, appId, name }) => ({ requestId, appId, name }));
    }
    if (action === 'authoringSessions') {
      const sessions = await this.authoringSessionIntents.sessions();
      const owners = new Map(await Promise.all([...new Set(sessions.map(row => row.projectPath))].map(async path =>
        [path, (await this.store.findAuthoringSource(path))?.ownerAppId] as const)));
      return sessions.map(({ name, appId, projectPath, sessionId, source }) =>
        ({ name, appId: appId ?? owners.get(projectPath), projectPath, sessionId, source }));
    }
    if (action === 'resumeAuthoringSession') {
      if (typeof input.requestId !== 'string' || !UUID_PATTERN.test(input.requestId)) throw new Error('Invalid Local App session intent');
      const saved = await this.getAuthoringSession(input.requestId);
      await this.options.edit({ id: saved.appId ?? '', name: saved.name, authoringSessionIntentId: saved.requestId });
      return;
    }
    if (action === 'resumePublication') {
      if (typeof input.requestId !== 'string' || !UUID_PATTERN.test(input.requestId)) throw new Error('Invalid Local App publication request');
      const saved = await this.store.publicationIntent(input.requestId);
      if (!saved || saved.state === 'completed') throw new Error('The pending Local App publication is no longer available');
      const row = await this.runtime.exclusive(saved.appId ?? '__publish_new__', () => this.store.resumePublication(saved.requestId));
      this.invalidateRemoteOpenings(row.id); this.remoteGateway?.revokeApp(row.id); this.changed();
      return row;
    }
    if (action === 'resumeSourceCopy') {
      if (typeof input.requestId !== 'string' || !UUID_PATTERN.test(input.requestId)) throw new Error('Invalid Local App source operation');
      return this.withAuthoringSourceLock(async () => {
        const saved = await this.authoringSourceIntents.getCopy(input.requestId as string);
        if (!saved || saved.state === 'completed') throw new Error('The pending Local App source operation is no longer available');
        const ready = await this.completeSourceCopy(saved);
        const row = await this.store.get(ready.targetAppId!);
        await this.options.edit({ id: ready.targetAppId!, name: row.name, authoring: { sourceDirectory: ready.sourcePath },
          authoringAction: 'rebindRequired', authoringSetCurrent: false, authoringRequestId: ready.requestId });
      });
    }
    if (action === 'prepareAuthoring') {
      const session = sourceSessionRef(input.session);
      if (!session) throw new Error('Choose the new Local App Code session before creating its draft');
      return await this.prepareNativeAuthoring(session, origin.kind === 'native' ? origin.authoringSource : undefined);
    }
    if (action === 'bindAuthoring') {
      const id = appId(input.id), sessionRef = sourceSessionRef(input.session);
      if (!sessionRef || typeof input.setCurrent !== 'boolean') throw new Error('Choose an app Code session and its authoring intent');
      if (input.sourceRequestId !== undefined && (typeof input.sourceRequestId !== 'string' || !UUID_PATTERN.test(input.sourceRequestId))) {
        throw new Error('Invalid Local App source operation');
      }
      const order = input.setCurrent ? (origin.kind === 'native' ? origin.authoringOrder : undefined) ?? ++this.authoringSequence : undefined;
      const finishVerification = input.setCurrent ? this.trackAuthoringVerification(id) : undefined;
      let intent: ReturnType<LocalAppHost['beginAuthoringIntent']> | undefined;
      try {
        let resolved: Omit<DesktopAuthoringRef, 'workspaceId' | 'appId'>;
        try {
          resolved = await this.resolveNativeAuthoring(sessionRef, origin.kind === 'native' ? origin.authoringSource : undefined);
          intent = input.setCurrent ? this.beginAuthoringIntent(id, order) : undefined;
        } finally { finishVerification?.(); }
        const operation = { purpose: 'rebind' as const, sourceDirectory: resolved.sourceDirectory,
          projectPath: resolved.projectRef.path, sessionId: resolved.authoringSessionId, appId: id };
        let requestId: string;
        let found: DesktopAuthoringSourceProof | null = null;
        const copy = input.sourceRequestId === undefined ? undefined : await this.authoringSourceIntents.getCopy(input.sourceRequestId as string);
        if (input.sourceRequestId !== undefined && (!copy || copy.state === 'pending'
          || copy.targetAppId !== id || copy.sourcePath !== resolved.sourceDirectory)) {
          throw new Error('The Local App source operation does not match this authoring session');
        }
        if (copy) {
          requestId = copy.requestId;
          found = await this.store.findAuthoringSource(resolved.sourceDirectory);
          if (!found) throw new Error('The Local App source proof is unavailable');
          this.checkAuthoringSource(found, resolved.sourceDirectory);
          if (found.sourceId !== copy.sourceId || (found.ownerAppId !== undefined && found.ownerAppId !== id)) {
            throw new Error('The Local App source proof changed');
          }
        } else if (input.setCurrent || (origin.kind === 'native' && origin.recoverExplicitBinding)) {
          requestId = await this.authoringSourceIntents.sourceRequestId(operation);
        } else {
          found = await this.store.findAuthoringSource(resolved.sourceDirectory);
          if (!found) throw new Error('Choose the Local App source explicitly before repairing its authoring session');
          this.checkAuthoringSource(found, resolved.sourceDirectory);
          if (found.ownerAppId !== undefined && found.ownerAppId !== id) throw new Error('The Local App source belongs to another application');
          const existingIntent = await this.authoringSourceIntents.findSourceRequestId(operation);
          if (found.ownerAppId !== id && !existingIntent) throw new Error('Choose the Local App source explicitly before repairing its authoring session');
          requestId = existingIntent ?? await this.authoringSourceIntents.sourceRequestId({ ...operation, purpose: 'repair' });
        }
        const proof = copy ? found! : await this.store.ensureAuthoringSource(resolved.sourceDirectory, resolved.projectRef, requestId, id);
        if (!copy) this.checkAuthoringSource(proof, resolved.sourceDirectory, requestId, id);
        if (found && proof.sourceId !== found.sourceId) throw new Error('The Local App source identity changed during authoring recovery');
        const session = { projectPath: resolved.projectRef.path, sessionId: resolved.authoringSessionId };
        const saved = await this.runtime.exclusive(id, async () => {
          const current = await this.store.get(id);
          const previous = await this.existingAuthoringGrant(resolved.sourceDirectory, session);
          if (previous) this.checkAuthoringGrant(previous, proof.sourceId, session);
          if (previous?.ownerAppId !== undefined && previous.ownerAppId !== id) throw new Error('The Local App authoring receipt belongs to another application');
          const binding: DesktopAuthoringRef = { ...resolved, appId: id,
            workspaceId: previous?.authoring.workspaceId
              ?? (isDesktopAuthoringRef(current.authoring) && current.authoring.sourceDirectory === resolved.sourceDirectory
                && current.authoring.projectRef.path === resolved.projectRef.path ? current.authoring.workspaceId : randomUUID()) };
          const registered = await this.store.registerAuthoring(proof.sourceId, requestId, binding, intent?.current() === true);
          this.checkAuthoringGrant({ sourceId: proof.sourceId, authoring: registered, sessionMode: registered.sessionMode }, proof.sourceId, session);
          if (registered.workspaceId !== binding.workspaceId || (registered.appId !== undefined && registered.appId !== id)) {
            throw new Error('The Local App authoring receipt changed during registration');
          }
          if (copy) await this.authoringSourceIntents.completeCopy(copy.requestId);
          return registered;
        });
        this.changed();
        return saved;
      } finally { intent?.finish(); }
    }
    if (action === 'agent-questions') return [...this.agentQuestions.values()].map(question => question.view);
    if (action === 'agent-answer') {
      const pending = this.agentQuestions.get(String(input.questionId));
      if (!pending) throw new Error('This Agent question is no longer active');
      if (input.answers === null) pending.resolve(null);
      else {
        if (!input.answers || typeof input.answers !== 'object' || Array.isArray(input.answers)) throw new Error('Invalid Agent answers');
        const answers = input.answers as Record<string, unknown>;
        const result: Record<string, string> = Object.create(null);
        for (const question of pending.view.request.questions) {
          const value = answers[question.question];
          if (typeof value !== 'string' || !value.trim() || value.length > 16000) throw new Error('Answer each Agent question');
          result[question.question] = value;
        }
        pending.resolve(result);
      }
      return;
    }
    if (action === 'publish') {
      const key = input.id === undefined ? '__publish_new__' : appId(input.id);
      const publish = {
        ...(typeof input.id === 'string' ? { id: input.id } : {}),
        ...(typeof input.name === 'string' ? { name: input.name } : {}),
        ...(typeof input.summary === 'string' ? { summary: input.summary } : {}),
        ...(typeof input.html === 'string' ? { html: input.html } : {}),
        ...(typeof input.staged_path === 'string' ? { staged_path: input.staged_path } : {}),
        ...(typeof input.expectedRevision === 'number' ? { expectedRevision: input.expectedRevision } : {}),
      };
      const row = await this.withAuthoringSourceLock(() => this.runtime.exclusive(key, async () => {
          const verified = committedAuthoring === undefined
            ? await this.publishAuthoring(input, typeof input.id === 'string' ? appId(input.id) : undefined, originatingAuthoring!, publicationRequestId!)
            : committedAuthoring ?? undefined;
          // Publishing an existing app records its actual publisher; only an
          // explicit rebind changes that app's preferred authoring owner.
          const setCurrent = priorPublication?.native?.setCurrent ?? (input.id === undefined);
          const published = await this.store.publish({ ...publish,
            publication: { requestId: publicationRequestId!, sourceDirectory: originatingAuthoring!.sourceDirectory,
              authoringSessionId: originatingAuthoring!.authoringSessionId },
            ...(committedAuthoring !== undefined ? { publicationReplayOnly: true } : {}),
            ...(verified ? { nativeAuthoring: { ...verified, setCurrent } } : {}) });
          return published;
        }));
      this.invalidateRemoteOpenings(row.id); this.remoteGateway?.revokeApp(row.id);
      this.changed();
      return { action, id: row.id, revision: row.revision, name: row.name, summary: row.summary, runtime: 'desktop', warnings: [], opensFrom: 'chat card or Apps library' };
    }
    // The creating chat may discover public tool IDs before writing a manifest.
    // This read-only catalog never returns private connection config or grants.
    if (action === 'agent_catalog') {
      const catalog = await this.options.agentCatalog?.();
      if (!catalog) throw new Error('Application Agent host is unavailable');
      return catalog;
    }
    const id = appId(input.id);
    if (action === 'versions') return this.store.core.request({ op: 'releases', appId: id });
    if (action === 'useVersion') {
      if (!Number.isSafeInteger(input.revision) || Number(input.revision) < 1 || !Number.isSafeInteger(input.expectedCurrentRevision) || Number(input.expectedCurrentRevision) < 1) throw new Error('Choose a version from the current release list');
      this.invalidateRemoteOpenings(id); this.remoteGateway?.revokeApp(id);
      const epoch = await this.cancelOpeningBeforeNext(id, signal);
      this.assertOpeningEpoch(id, epoch, signal);
      await this.open(id, false, true, signal, Number(input.revision), Number(input.expectedCurrentRevision));
      this.changed(); return;
    }
    if (action === 'openData') {
      await this.store.get(id);
      const error = await shell.openPath(this.store.dataPath(id));
      if (error) throw new Error(error);
      return;
    }
    if (action === 'restoreSource' || action === 'remix') {
      const intent = this.beginAuthoringIntent(id);
      try {
        const row = await this.store.get(id), sourcePath = await this.authoringCopyPath(row.name);
        const copy = await this.authoringSourceIntents.createCopy({ purpose: action, appId: id, revision: row.revision, sourcePath });
        const ready = await this.completeSourceCopy(copy);
        const target = await this.store.get(ready.targetAppId!);
        this.changed();
        if (intent.current()) await this.options.edit({ id: ready.targetAppId!, name: target.name, authoring: { sourceDirectory: ready.sourcePath },
          authoringAction: 'rebindRequired', authoringSetCurrent: true, authoringRequestId: ready.requestId });
        return;
      } finally { intent.finish(); }
    }
    if (action === 'marketPublish') {
      const metadata = input.metadata as MarketPublication;
      if (!metadata || typeof metadata.name !== 'string' || typeof metadata.summary !== 'string' || !metadata.summary.trim()) throw new Error('Enter a name and summary before publishing');
      const prepared = await this.market.preparePublication(id, Number(input.expectedRevision));
      try {
        const options = { type: 'question' as const, message: `Publish ${metadata.name} source to the market?`, detail: `${prepared.files.length} explicitly included files:\n${prepared.files.map(file => file.path).join('\n')}\n\nThe source will be uploaded to your account. Services and new capabilities may require review. Local permissions and data are not included.`, buttons: ['Cancel', 'Upload source'], defaultId: 0, cancelId: 0 };
        const parent = this.options.mainWindow();
        const choice = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
        if (choice.response !== 1) return;
        return await this.market.publish(prepared, metadata);
      } finally { await fs.rm(prepared.directory, { recursive: true, force: true }); }
    }
    if (action === 'status') {
      const { authoring: _authoring, authoringSourceMissing: _missing, authoringAction: _action, ...row } = this.runtime.status(await this.store.get(id));
      return { action, ...row };
    }
    if (action === 'read') {
      const row = this.runtime.status(await this.store.get(id));
      const sourceSnapshotPath = this.store.revisionPath(id, row.revision);
      let sourceDirectory = row.authoring?.sourceDirectory;
      if (sourceDirectory) { try { await fs.access(sourceDirectory); } catch { sourceDirectory = undefined; } }
      return { action, ...row, authoringSourceMissing: Boolean(row.authoring?.sourceDirectory && !sourceDirectory), sourceSnapshotPath, expectedRevision: row.revision };
    }
    if (action === 'logs') return { action, id, logs: this.runtime.logs(id) };
    if (action === 'agent-catalog') {
      const row = await this.store.get(id);
      if (!row.manifest.capabilities?.includes('agent')) throw new Error('This application does not request Agent access');
      const catalog = await this.options.agentCatalog?.();
      if (!catalog) throw new Error('Application Agent host is unavailable');
      return { ...catalog, tools: catalog.tools.filter(tool => row.manifest.agent?.tools.includes(tool.id)),
        skills: (row.manifest.agent?.skills ?? []).map(skill => ({ id: skill.id, label: skill.id })) };
    }
    if (action === 'agent-settings') {
      const row = await this.store.get(id);
      if (!row.manifest.capabilities?.includes('agent')) throw new Error('This application does not request Agent access');
      const settings = input.settings as LocalAppAgentSettings;
      if (!settings || typeof settings.enabled !== 'boolean' || typeof settings.background !== 'boolean') throw new Error('Invalid Agent settings');
      // Revocation must remain possible when a model or connector disappeared.
      if (!settings.enabled) {
        await this.cancelAgentApp(id, 'Application Agent access disabled');
        await this.runtime.exclusive(id, async () => {
          const current = await this.store.get(id);
          if (current.agent) current.agent = { ...current.agent, enabled: false, tools: [], skills: [], background: false };
          current.grants = current.grants.filter(cap => cap !== 'agent');
          this.launchGrants.set(id, (this.launchGrants.get(id) ?? []).filter(cap => cap !== 'agent'));
          await this.store.save(current);
        });
        if (!this.windows.has(id) && !row.background && !this.retainRemoteRun(id)) await this.runtime.stop(id);
        this.changed(); return;
      }
      validateAgentSettings({ ...settings, enabled: true });
      const catalog = await this.options.agentCatalog?.();
      if (!catalog) throw new Error('Application Agent host is unavailable');
      if (settings.enabled && !catalog.models.some(model => model.canSelect && model.providerId === settings.model.providerId && model.modelId === settings.model.modelId)) throw new Error('Select an available Agent model');
      const tools = settings.tools.map(grant => {
        const found = catalog.tools.find(tool => sameAgentGrant(grant, tool));
        if (!found || !row.manifest.agent?.tools.includes(found.id)) throw new Error('Only current tools requested by this application can be granted');
        // Persist only permission identity, never labels or catalog metadata.
        return found.kind === 'builtin' ? { id: found.id, kind: found.kind, name: found.name } : { id: found.id, kind: found.kind, serverConfigId: found.serverConfigId, configRevision: found.configRevision, rawToolName: found.rawToolName, definitionDigest: found.definitionDigest };
      });
      const requestedSkills = (row.manifest.agent?.skills ?? []).map(skill => skill.id);
      if (!Array.isArray(settings.skills) || settings.skills.some(skill => !requestedSkills.includes(skill))) throw new Error('Only Skills declared by this release can be granted');
      const epoch = await this.cancelAgentApp(id, 'Application Agent permissions changed');
      try {
        await this.runtime.exclusive(id, async () => {
          const current = await this.store.get(id);
          if (current.revision !== row.revision) throw new Error('Application changed; reopen Agent settings');
          current.agent = { enabled: settings.enabled, model: { providerId: settings.model.providerId, modelId: settings.model.modelId }, tools, skills: [...settings.skills], background: settings.background, maxTurns: settings.maxTurns, timeoutMs: settings.timeoutMs };
          current.grants = settings.enabled ? [...new Set([...current.grants, 'agent' as const])] : current.grants.filter(cap => cap !== 'agent');
          await this.store.save(current);
          if (this.runtime.manifest(id)?.capabilities?.includes('agent')) {
            const grants = this.launchGrants.get(id) ?? [];
            this.launchGrants.set(id, settings.enabled ? [...new Set([...grants, 'agent' as const])] : grants.filter(cap => cap !== 'agent'));
          }
        });
      } finally { if (this.agentEpochs.get(id) === epoch && this.runtime.phase(id) === 'running') this.blockedAgentApps.delete(id); }
      if (!this.windows.has(id) && !row.background && (!settings.enabled || !settings.background) && !this.retainRemoteRun(id)) await this.runtime.stop(id);
      this.changed(); return;
    }
    if (action === 'open' || action === 'start') { await this.open(id, false, false, signal); return { action, id, phase: this.runtime.status(await this.store.get(id)).phase }; }
    if (action === 'stop') {
      await this.stopManagedApp(id, 'Application stopped');
    } else if (action === 'restart' || action === 'repair') {
      const epoch = await this.cancelOpeningBeforeNext(id, signal);
      this.assertOpeningEpoch(id, epoch, signal);
      await this.open(id, action === 'repair', true);
    } else if (action === 'background') {
      if (typeof input.enabled !== 'boolean') throw new Error('Invalid background preference');
      await this.runtime.exclusive(id, async () => { const row = await this.store.get(id); row.background = input.enabled as boolean; await this.store.save(row); });
      if (!input.enabled && !this.windows.has(id) && !this.opening.has(id) && !this.retainRemoteRun(id)) await this.runtime.stop(id);
    } else if (action === 'revoke') {
      this.launchGrants.delete(id);
      await this.stopManagedApp(id, 'Application permissions revoked', () => this.runtime.exclusive(id, async () => {
        await this.store.revoke(id); await fs.rm(join(this.store.directory(id), 'file-grants.json'), { force: true });
      }), true);
    } else if (action === 'remove') {
      const parent = this.options.mainWindow();
      const options = { type: 'question' as const, message: nativeTr('native_delete_app', (await this.store.get(id)).name), detail: nativeTr('native_delete_app_detail'), buttons: [nativeTr('common_cancel'), nativeTr('common_delete')], defaultId: 0, cancelId: 0 };
      const choice = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
      if (choice.response !== 1) return;
      await this.stopManagedApp(id, 'Application deleted', async () => {
        await this.setRemoteAccess(id, false);
        this.windows.get(id)?.destroy();
        const partition = session.fromPartition(`persist:local-app-${id}`);
        await partition.clearStorageData();
        await partition.clearCache();
        partition.flushStorageData();
        await this.runtime.remove(id);
      });
    } else if (action === 'edit') {
      const intent = this.beginAuthoringIntent(id);
      try {
        const row = await this.store.get(id);
        if (row.marketOrigin && !row.authoring?.sourceDirectory) throw new Error('Create a permitted custom copy before developing a market application');
        if (intent.current()) await this.editAuthoring(row, false, intent.current);
      } finally { intent.finish(); }
    }
    else throw new Error('Unknown Local App action');
    this.changed(); return { action, id };
  }
  private async stopManagedApp(id: string, reason: string, finish?: () => Promise<void>, finishOnCleanupFailure = false): Promise<void> {
    const release = this.runtime.suspendStarts(id);
    this.invalidateRemoteOpenings(id); this.remoteGateway?.revokeApp(id);
    const opening = this.cancelOpening(id);
    try {
      const results = await Promise.allSettled([this.mcp.revoke(id), this.cancelAgentApp(id, reason)]);
      const errors = results.filter((result): result is PromiseRejectedResult => result.status === 'rejected').map(result => result.reason);
      try { await this.runtime.stop(id); } catch (error) { errors.push(error); }
      await opening;
      this.windows.get(id)?.close();
      // Revocation must persist even when process cleanup needs retry. Deletion waits for cleanup.
      if (finish && (!errors.length || finishOnCleanupFailure)) try { await finish(); } catch (error) { errors.push(error); }
      if (errors.length) throw new AggregateError(errors, 'Application cleanup needs retry');
    } finally { release(); }
  }
  private async authoringCopyPath(name: string): Promise<string> {
    const root = this.options.appProjectsRoot; await fs.mkdir(root, { recursive: true, mode: 0o700 });
    return join(await fs.realpath(root), `${name.replace(/[^\p{L}\p{N} _-]/gu, '').slice(0, 50) || 'Application'}-${randomUUID().slice(0, 8)}`);
  }
  private cancelOpening(id: string): Promise<void> {
    this.openingEpochs.set(id, (this.openingEpochs.get(id) ?? 0) + 1);
    const opening = this.opening.get(id);
    opening?.controller.abort();
    for (const controller of this.waitingOpenings.get(id) ?? []) controller.abort();
    // Callers must also wait for open() to release its promise and cleanup, so
    // the next Open cannot join an operation that has already been canceled.
    return opening?.promise.then(() => undefined, () => undefined) ?? Promise.resolve();
  }
  private async cancelOpeningBeforeNext(id: string, signal?: AbortSignal): Promise<number> {
    const opening = this.cancelOpening(id);
    const epoch = this.openingEpochs.get(id)!;
    if (signal) await withAbort(opening, signal); else await opening;
    this.assertOpeningEpoch(id, epoch, signal);
    return epoch;
  }
  private assertOpeningEpoch(id: string, epoch: number, signal?: AbortSignal): void {
    signal?.throwIfAborted();
    if (this.shuttingDown || this.unavailable || this.openingEpochs.get(id) !== epoch) throw new Error('Application opening cancelled');
  }
  private async authorize(id: string, signal: AbortSignal, revision?: number): Promise<void> {
    signal.throwIfAborted();
    const row = revision === undefined ? await this.store.get(id) : await this.store.getRevision(id, revision);
    const missing = (row.manifest.capabilities ?? []).filter(c => c !== 'agent' && !row.grants.includes(c));
    if (!missing.length) return;
    const labels: Record<LocalAppCapability, string> = {
      process: nativeTr('native_capability_process'),
      mcp: 'Expose this application’s tools through standard MCP while it is running',
      files: nativeTr('native_capability_files'),
      clipboard: nativeTr('native_capability_clipboard'),
      notifications: nativeTr('native_capability_notifications'),
      camera: nativeTr('native_capability_camera'),
      microphone: nativeTr('native_capability_microphone'),
      photos: 'Select photos',
      share: 'Share files',
      agent: 'Call an independent Agent using the model and individual tools you allow in Apps',
      background: 'Continue running in the background',
      network: 'Use network connections',
      location: 'Use location',
      calendar: 'Use calendar',
      contacts: 'Use contacts',
      haptics: 'Use haptics',
    };
    const options = { type: 'question' as const, signal, title: row.name, message: nativeTr('native_allow_capabilities', row.name),
      detail: missing.map(c => labels[c]).join('\n') + '\n\n' + nativeTr('native_revoke_hint'), buttons: [nativeTr('common_cancel'), nativeTr('permission_allow')], defaultId: 0, cancelId: 0 };
    const parent = this.options.mainWindow();
    signal.throwIfAborted();
    const result = parent ? await dialog.showMessageBox(parent, options) : await dialog.showMessageBox(options);
    signal.throwIfAborted();
    if (result.response !== 1) throw new Error('Application permission was not granted');
    await this.runtime.exclusive(id, async () => {
      signal.throwIfAborted();
      const current = await this.store.getRevision(id, row.revision);
      if (current.releaseDigest !== row.releaseDigest) throw new Error('Application changed while permission was being reviewed');
      current.grants = [...new Set([...current.grants, ...missing])];
      await this.store.save(current);
    });
  }
  async open(id: string, repair = false, restart = false, signal?: AbortSignal, revision?: number, expectedCurrentRevision?: number): Promise<void> {
    if (this.shuttingDown) throw new Error('Application host is stopping');
    signal?.throwIfAborted();
    const intent = JSON.stringify([repair, restart, revision ?? null, expectedCurrentRevision ?? null]);
    const pending = this.opening.get(id);
    if (pending && (pending.intent !== intent || pending.controller.signal.aborted)) {
      const controller = new AbortController();
      const waiting = this.waitingOpenings.get(id) ?? new Set<AbortController>();
      waiting.add(controller); this.waitingOpenings.set(id, waiting);
      const waitingSignal = signal ? AbortSignal.any([signal, controller.signal]) : controller.signal;
      try {
        await withAbort(pending.promise.catch(() => undefined), waitingSignal);
        waitingSignal.throwIfAborted();
        return this.open(id, repair, restart, signal, revision, expectedCurrentRevision);
      } finally {
        waiting.delete(controller); if (!waiting.size) this.waitingOpenings.delete(id);
      }
    }
    if (pending) return this.joinOpening(pending, signal);
    const controller = new AbortController();
    const previousInvocation = this.runtime.invocationId(id);
    const operation = { controller, intent, waiters: 0, promise: this.openOnce(id, controller.signal, repair, restart, revision, expectedCurrentRevision) };
    this.opening.set(id, operation);
    operation.promise = operation.promise.catch(async error => {
      if (!this.shuttingDown && ['running', 'starting', 'installing', 'building'].includes(this.runtime.phase(id))) {
        const ownsNewRun = this.runtime.invocationId(id) !== previousInvocation;
        const orphanedForeground = !this.windows.has(id) && !(await this.store.get(id)).background;
        if ((ownsNewRun || orphanedForeground) && !this.retainRemoteRun(id)) await this.runtime.stop(id);
      }
      throw error;
    }).finally(() => {
      if (this.opening.get(id) === operation) this.opening.delete(id);
    });
    await this.joinOpening(operation, signal);
  }
  private async joinOpening(opening: { promise: Promise<void>; controller: AbortController; waiters: number }, signal?: AbortSignal): Promise<void> {
    opening.waiters++;
    let joined = true;
    const leave = () => {
      if (!joined) return;
      joined = false;
      opening.waiters--;
      if (!opening.waiters) opening.controller.abort();
    };
    // Propagate the last cancellation synchronously, before an already queued
    // window-load continuation can commit the abandoned opening.
    let cancel!: () => void;
    const cancelled = signal && new Promise<never>((_resolve, reject) => {
      cancel = () => {
        signal.removeEventListener('abort', cancel);
        leave();
        reject(signal.reason);
      };
      signal.addEventListener('abort', cancel, { once: true });
      if (signal.aborted) cancel();
    });
    try {
      if (cancelled) await Promise.race([opening.promise, cancelled]);
      else await opening.promise;
    } finally {
      if (signal) signal.removeEventListener('abort', cancel);
      leave();
    }
  }
  private async openOnce(id: string, signal: AbortSignal, repair: boolean, restart: boolean, revision?: number, expectedCurrentRevision?: number): Promise<void> {
    const cleaning = this.remoteCleaning.get(id);
    if (cleaning) await withAbort(cleaning, signal);
    signal.throwIfAborted();
    let agentEpoch = this.agentEpochs.get(id);
    const row = revision === undefined ? await this.store.get(id) : await this.store.getRevision(id, revision);
    const existing = this.windows.get(id);
    if (existing && !existing.isDestroyed() && !restart && this.runtime.phase(id) === 'running' && this.runtime.revision(id) === row.revision) { existing.show(); existing.focus(); return; }
    await this.store.assertTarget(row);
    await this.authorize(id, signal, revision);
    signal.throwIfAborted();
    if (this.runtime.revision(id) !== row.revision || repair || restart) agentEpoch = await this.cancelAgentApp(id, 'Application invocation changed');
    try {
      const backgroundOwner = this.backgroundAgentOwners.get(id);
      if (backgroundOwner && backgroundOwner.revision === row.revision && !repair && !restart) await this.runtime.resume(id, backgroundOwner.revision, backgroundOwner.invocationId, signal);
      else await this.runtime.start(id, repair, signal, restart || this.runtime.revision(id) !== row.revision, revision, expectedCurrentRevision);
    } catch (error) {
      if (this.agentEpochs.get(id) === agentEpoch && this.runtime.phase(id) === 'running') this.blockedAgentApps.delete(id);
      throw error;
    }
    const invocation = this.runtime.invocationId(id);
    // A replacement gets a fresh document host. Old pages cannot acquire the new run's APIs.
    if (existing && !existing.isDestroyed()) existing.destroy();
    const assertRunning = () => {
      signal.throwIfAborted();
      if (this.runtime.invocationId(id) !== invocation) throw new Error('Application changed while opening');
      const current = this.runtime.status(row);
      if (current.phase !== 'running') throw new Error(current.error ?? 'Application stopped while opening');
    };
    assertRunning();
    if (this.agentEpochs.get(id) === agentEpoch) this.blockedAgentApps.delete(id);
    this.launchGrants.set(id, (await this.store.getRevision(id, this.runtime.revision(id))).grants.filter(cap => this.runtime.manifest(id)?.capabilities?.includes(cap)));
    await this.configureSession(id);
    await this.clearCodeCache(id);
    assertRunning();
    const entryUrl = appEntryUrl(id, this.runtime.manifest(id)!.entry);
    const window = new BrowserWindow({ width: 1080, height: 760, minWidth: 360, minHeight: 320, title: row.name, show: false,
      webPreferences: { partition: `persist:local-app-${id}`, preload: this.options.preload, contextIsolation: true, nodeIntegration: false, sandbox: true, webSecurity: true, navigateOnDragDrop: false } });
    this.windows.set(id, window);
    this.windowRuns.set(window, invocation!);
    window.webContents.on('did-start-navigation', (_event, _url, inPlace, mainFrame) => { if (mainFrame && !inPlace) this.navigationEpoch.set(window.webContents.id, (this.navigationEpoch.get(window.webContents.id) ?? 0) + 1); });
    const contentsId = window.webContents.id;
    const origin = `${LOCAL_APP_SCHEME}://${id}`;
    window.webContents.on('will-navigate', (event, url) => { if (!url.startsWith(origin + '/')) { event.preventDefault(); void this.external(url); } });
    window.webContents.setWindowOpenHandler(({ url }) => { void this.external(url); return { action: 'deny' }; });
    window.webContents.on('will-attach-webview', event => event.preventDefault());
    window.on('closed', () => {
      const closingInvocation = this.runtime.invocationId(id), closingRevision = this.runtime.revision(id);
      this.navigationEpoch.delete(contentsId);
      if (this.windows.get(id) === window) this.windows.delete(id);
      if (!this.shuttingDown && closingInvocation) void this.queueRunCleanup(id, () => this.desktopWindowClosed(id, closingInvocation, closingRevision)).catch(() => undefined);
    });
    try { await window.loadURL(entryUrl); assertRunning(); window.show(); }
    catch (error) { window.destroy(); if (!this.retainRemoteRun(id)) await this.runtime.stop(id); throw error; }
  }
  private async desktopWindowClosed(id: string, invocation: string, revision?: number): Promise<void> {
    const unused = () => !this.shuttingDown && this.runtime.invocationId(id) === invocation && !this.windows.has(id) && !this.opening.has(id) && !this.retainRemoteRun(id);
    const row = await this.store.get(id);
    if (!unused()) return;
    if (row.agent?.enabled && row.agent.background && revision) this.backgroundAgentOwners.set(id, { revision, invocationId: invocation });
    else await this.cancelAgentApp(id, 'Application window closed');
    const current = await this.store.get(id);
    if (!current.background && unused()) await this.runtime.stop(id);
  }
  private async external(raw: string): Promise<void> { try { const url = new URL(raw); if (['https:', 'http:', 'mailto:'].includes(url.protocol)) await shell.openExternal(url.toString()); } catch { /* invalid URL */ } }
  private async clearCodeCache(id: string): Promise<void> {
    const partition = session.fromPartition(`persist:local-app-${id}`);
    await partition.clearCache();
    // CacheStorage can contain user-created offline documents. Refresh code
    // without deleting origin data owned by the application.
    await partition.clearStorageData({ storages: ['serviceworkers'] });
  }
  private async configureSession(id: string): Promise<void> {
    if (this.configured.has(id)) return;
    const partition = session.fromPartition(`persist:local-app-${id}`);
    const permission = permissionCapabilities;
    partition.setPermissionCheckHandler((contents, kind, origin, details) => {
      const window = this.windows.get(id);
      if (!contents || !window || contents !== window.webContents || this.windowRuns.get(window) !== this.runtime.invocationId(id) || !details.isMainFrame || !isAppUrl(id, origin) || this.runtime.phase(id) !== 'running') return false;
      // The synchronous check only admits capabilities approved for this launch.
      return permission(kind, details).length > 0 && permission(kind, details).every(c => this.launchGrants.get(id)?.includes(c));
    });
    partition.setPermissionRequestHandler(async (contents, kind, callback, details) => {
      try {
        const window = this.windows.get(id), invocation = window ? this.windowRuns.get(window) : undefined;
        const current = () => contents === this.windows.get(id)?.webContents && !contents.isDestroyed() && details.isMainFrame && isAppUrl(id, details.requestingUrl) && isAppUrl(id, contents.getURL()) && this.runtime.phase(id) === 'running' && this.runtime.invocationId(id) === invocation;
        if (!current()) { callback(false); return; }
        const row = await this.store.getRevision(id, this.runtime.revision(id)), required = permission(kind, details);
        let allowed = required.length > 0 && required.every(c => row.grants.includes(c) && this.launchGrants.get(id)?.includes(c));
        if (allowed && process.platform === 'darwin') for (const c of required) if (c === 'microphone' || c === 'camera') allowed = await systemPreferences.askForMediaAccess(c) && allowed;
        callback(allowed && current() && required.every(c => this.launchGrants.get(id)?.includes(c)));
      } catch { callback(false); }
    });
    partition.protocol.handle(LOCAL_APP_SCHEME, async request => {
      try {
        const url = new URL(request.url);
        if (url.hostname !== id || url.username || url.password) return new Response('Not found', { status: 404 });
        const row = await this.store.get(id);
        const revision = this.runtime.revision(id);
        const workspace = this.runtime.workspace(id);
        const window = this.windows.get(id);
        if (!window || window.isDestroyed() || this.windowRuns.get(window) !== this.runtime.invocationId(id) || !workspace || !revision || this.runtime.phase(id) !== 'running') return new Response('Application run is unavailable', { status: 410 });
        const manifest = this.runtime.manifest(id)!;
        if (url.pathname === '/__lingxi_local_app_sdk.js') {
          return new Response(LOCAL_APP_SDK, { headers: { 'Content-Type': 'text/javascript; charset=utf-8', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' } });
        }
        if (manifest.entry.type === 'server' && url.pathname === '/') {
          const landing = new URL(appEntryUrl(id, manifest.entry));
          if (landing.pathname !== '/') {
            landing.search = url.search;
            return new Response(null, { status: 307, headers: { Location: landing.href, 'Cache-Control': 'no-store' } });
          }
        }
        let response: Response;
        let serviceResponse = false;
        const serviceRoute = /^\/__services\/([\w-]+)(\/.*)?$/.exec(url.pathname);
        if (manifest.entry.type === 'server' || serviceRoute) {
          const service = this.runtime.service(id, serviceRoute?.[1] ?? (manifest.entry.type === 'server' ? manifest.entry.process : ''));
          if (!service) return new Response('Backend is unavailable', { status: 503 });
          const path = serviceRoute ? serviceRoute[2] ?? '/' : url.pathname;
          const runId = this.runtime.invocationId(id)!;
          const useCookies = request.credentials !== 'omit' && request.headers.get(LOCAL_APP_CREDENTIALS_HEADER) !== 'omit';
          const cookiePrefix = serviceRoute ? `/__services/${serviceRoute[1]}` : '';
          // Chromium cannot persist cookies for this custom scheme. Read and
          // write them through the owning App's partition, keyed by its public
          // request path, while never forwarding a page-supplied Cookie header.
          const cookieHeader = useCookies ? await serviceCookieHeader(partition.cookies, url.pathname).catch(() => undefined) : undefined;
          response = await this.services.fetch(id, runId, serviceRoute?.[1] ?? (manifest.entry.type === 'server' ? manifest.entry.process : ''), service.port, path + url.search, request, cookieHeader ?? '');
          if (useCookies) await storeServiceCookies(partition.cookies, path, cookiePrefix, response.headers);
          serviceResponse = true;
        } else {
          const assetRoot = dirname(manifest.entry.path);
          const path = url.pathname === '/' ? manifest.entry.path : join(assetRoot, decodeURIComponent(url.pathname).replace(/^\/+/, '')).split(sep).join('/');
          let file: string;
          try { file = await containedFile(workspace, path); }
          catch { if (request.headers.get('accept')?.includes('text/html')) file = await containedFile(workspace, manifest.entry.path); else throw new Error('Missing application asset'); }
          const assetHeaders = new Headers(request.headers);
          assetHeaders.delete(LOCAL_APP_CREDENTIALS_HEADER);
          response = await net.fetch(pathToFileURL(file).toString(), { headers: assetHeaders });
        }
        let body: BodyInit = response.body ?? '';
        const headers = new Headers(response.headers);
        if (serviceResponse) headers.delete('set-cookie');
        if (headers.get('content-type')?.toLowerCase().includes('text/html')) {
          const html = await response.text();
          body = injectLocalAppScripts(html, id, url.pathname);
          headers.delete('Content-Length');
          headers.delete('Content-Encoding');
          headers.set('Content-Type', headers.get('content-type') ?? 'text/html; charset=utf-8');
        }
        headers.set('X-Content-Type-Options', 'nosniff');
        // Desktop supports modules, workers, network, and the app's own servers.
        headers.set('Content-Security-Policy', "default-src 'self' data: blob: https: http: ws: wss:; script-src 'self' 'unsafe-inline' 'unsafe-eval' blob: https: http:; style-src 'self' 'unsafe-inline' https: http:; worker-src 'self' blob:; object-src 'none'; frame-ancestors 'none'");
        headers.set('Cache-Control', 'no-store');
        return new Response(body, { status: response.status, statusText: response.statusText, headers });
      } catch { return new Response('Application resource unavailable', { status: 404 }); }
    });
    this.configured.add(id);
  }
  private readonly launchGrants = new Map<string, LocalAppCapability[]>();
  private async native(event: IpcMainInvokeEvent, input: { method: string; args?: Record<string, unknown> }): Promise<unknown> {
    if (!input || typeof input.method !== 'string' || (input.args !== undefined && (!input.args || typeof input.args !== 'object' || Array.isArray(input.args)))) throw new Error('Invalid application API request');
    const pair = [...this.windows].find(([, window]) => window.webContents === event.sender);
    if (!pair || event.senderFrame !== event.sender.mainFrame || !event.senderFrame?.url.startsWith(`${LOCAL_APP_SCHEME}://${pair[0]}/`)) throw new Error('Unauthorized application caller');
    const id = pair[0], invocation = this.windowRuns.get(pair[1]), epoch = this.navigationEpoch.get(event.sender.id), revision = this.runtime.revision(id), args = input.args ?? {};
    const agentEpoch = this.agentEpochs.get(id) ?? 0;
    if (!revision) throw new Error('Application operation is no longer active');
    const row = await this.store.getRevision(id, revision);
    const current = () => {
      if (!invocation || this.windows.get(id) !== pair[1] || pair[1].isDestroyed() || event.senderFrame !== event.sender.mainFrame || !isAppUrl(id, event.senderFrame?.url ?? '') || this.navigationEpoch.get(event.sender.id) !== epoch || this.runtime.invocationId(id) !== invocation || this.runtime.phase(id) !== 'running') throw new Error('Application operation is no longer active');
    };
    current();
    const need = (cap: LocalAppCapability) => { current(); if (!row.grants.includes(cap) || !this.launchGrants.get(id)?.includes(cap)) throw new Error(`Application capability not granted: ${cap}`); };
    if (input.method === 'info') return { id, name: row.name, runId: invocation, revision: this.runtime.revision(id), platform: process.platform, dataDirectory: this.store.dataPath(id) };
    if (input.method.startsWith('agent.')) {
      need('agent');
      if (this.blockedAgentApps.has(id) || (this.agentEpochs.get(id) ?? 0) !== agentEpoch) throw new Error('Application Agent access is no longer active');
      const broker = this.agentBroker, activeOwner = this.runtime.owner(id);
      if (!broker || !invocation || !activeOwner || activeOwner.releaseDigest !== row.releaseDigest) throw new Error('Application Agent host is unavailable');
      const owner: LocalAppAgentOwner = { appId: id, releaseDigest: activeOwner.releaseDigest, invocationId: invocation, webContentsId: event.sender.id };
      const assertCurrentAgent = () => {
        current(); need('agent');
        if (this.blockedAgentApps.has(id) || (this.agentEpochs.get(id) ?? 0) !== agentEpoch
          || this.runtime.revision(id) !== revision || this.runtime.owner(id)?.releaseDigest !== activeOwner.releaseDigest) {
          throw new Error('Application Agent access changed');
        }
      };
      const guardedRead = async <T>(operation: Promise<T>): Promise<T> => {
        try { const value = await operation; assertCurrentAgent(); return value; }
        catch (error) { assertCurrentAgent(); throw error; }
      };
      if (input.method === 'agent.tasks.start') return broker.run(owner, args.input);
      if (input.method === 'agent.tasks.get') return guardedRead(broker.get(owner, String(args.runId)));
      if (input.method === 'agent.tasks.list') return guardedRead(broker.list(owner, args));
      if (input.method === 'agent.tasks.events') return guardedRead(broker.events(owner, String(args.runId), args.afterSequence as number | undefined));
      if (input.method === 'agent.tasks.cancel') return broker.cancel(owner, String(args.runId));
      if (input.method === 'agent.chats.create' || input.method === 'agent.chats.creationStatus' || input.method === 'agent.chats.list') {
        // Electron serializes thrown IPC errors without custom fields. Return a
        // narrow structured result so the page can distinguish definitive
        // conflicts from an uncertain transport failure.
        try {
          const value = input.method === 'agent.chats.create' ? await broker.createChat(owner, args)
            : input.method === 'agent.chats.creationStatus' ? await broker.chatCreationStatus(owner, args) : await broker.listChats(owner, args);
          assertCurrentAgent();
          return { ok: true, value };
        } catch (error) {
          try { assertCurrentAgent(); } catch { return { ok: false, error: { code: 'host_unavailable', message: 'The Agent host is unavailable.' } }; }
          const codes = new Set(['invalid_chat', 'invalid_input', 'chat_create_conflict', 'chat_create_deleted', 'chat_storage_uncertain', 'agent_storage_unavailable']);
          const code = error instanceof LocalAppAgentError && codes.has(error.code) ? error.code : 'host_unavailable';
          return { ok: false, error: { code, message: code === 'host_unavailable' ? 'The Agent host is unavailable.' : (error as Error).message } };
        }
      }
      if (input.method === 'agent.chats.get') return guardedRead(broker.getChat(owner, String(args.chatId)));
      if (input.method === 'agent.chats.archive') return broker.archiveChat(owner, String(args.chatId));
      if (input.method === 'agent.chats.delete') return broker.deleteChat(owner, String(args.chatId));
      if (input.method === 'agent.chats.history') return guardedRead(broker.chatHistory(owner, String(args.chatId), args));
      if (input.method === 'agent.chats.send') return broker.sendChat(owner, String(args.chatId), args.input);
      if (input.method === 'agent.chats.cancel') return broker.cancelChat(owner, String(args.chatId));
      throw new Error('Unknown application Agent API');
    }
    if (input.method === 'serviceSocketUrl') {
      const service = this.runtime.service(id, String(args.name));
      if (!service) throw new Error('Backend is unavailable');
      const path = String(args.path ?? '/');
      if (!path.startsWith('/') || path.startsWith('//') || path.includes('\\')) throw new Error('Invalid service path');
      return this.services.socketUrl(id, invocation!, String(args.name), service.port, path, `${LOCAL_APP_SCHEME}://${id}`);
    }
    if (input.method === 'clipboard.read') { need('clipboard'); return clipboard.readText(); }
    if (input.method === 'clipboard.write') { need('clipboard'); if (typeof args.text !== 'string') throw new Error('Expected text'); clipboard.writeText(args.text); return; }
    if (input.method === 'notify') { need('notifications'); new Notification({ title: row.name, body: String(args.body ?? '') }).show(); return; }
    if (input.method === 'process.exec') { need('process'); if (typeof args.command !== 'string') throw new Error('Expected command'); return this.runtime.execute(id, args.command); }
    if (input.method === 'files.pick') {
      need('files');
      if (Object.keys(args).some(key => key !== 'multiple' && key !== 'accept') || (args.multiple !== undefined && typeof args.multiple !== 'boolean')) throw new Error('Invalid file picker options');
      const accept = fileAccept(args.accept);
      const filters = accept.extensions.length ? [{ name: 'Accepted files', extensions: accept.extensions }] : undefined;
      const selected = await dialog.showOpenDialog(pair[1], { properties: ['openFile', ...(args.multiple === true ? ['multiSelections' as const] : [])], ...(filters ? { filters } : {}) });
      current(); need('files');
      if (selected.canceled || !selected.filePaths.length) return { files: [] };
      const paths = selected.filePaths;
      if (paths.length > LOCAL_APP_AGENT_MAX_FILES || (args.multiple !== true && paths.length > 1)) throw new Error('Choose at most eight files');
      let total = 0;
      const files = [];
      for (const path of paths) {
        const before = await fs.lstat(path);
        if (before.isSymbolicLink() || !before.isFile() || before.size < 1 || before.size > LOCAL_APP_AGENT_MAX_FILE_BYTES) throw new Error('Each selected file must be a regular file of 25 MiB or smaller');
        const mediaType = mediaTypeForPath(path);
        if (!acceptedFile(path, mediaType, accept.tokens)) throw new Error('A selected file does not match the requested file types');
        total += before.size;
        if (total > LOCAL_APP_AGENT_MAX_TOTAL_FILE_BYTES) throw new Error('Selected files must total 50 MiB or less');
        const handle = await fs.open(path, fsConstants.O_RDONLY | (fsConstants.O_NOFOLLOW ?? 0));
        try {
          const opened = await handle.stat();
          if (!opened.isFile() || opened.dev !== before.dev || opened.ino !== before.ino || opened.size !== before.size) throw new Error('The selected file changed while opening');
          const bytes = await handle.readFile();
          const after = await fs.lstat(path);
          if (after.isSymbolicLink() || after.dev !== opened.dev || after.ino !== opened.ino || bytes.length !== opened.size) throw new Error('The selected file changed while reading');
          const exact = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
          files.push({ name: path.split(sep).at(-1) ?? 'attachment', mediaType, size: bytes.length, lastModified: after.mtimeMs, bytes: exact });
        } finally { await handle.close(); }
      }
      current(); need('files');
      return { files };
    }
    if (input.method === 'files.pickDirectory') {
      need('files'); const result = await dialog.showOpenDialog(pair[1], { properties: ['openDirectory', 'createDirectory'] });
      if (result.canceled || !result.filePaths[0]) return null;
      const token = randomUUID();
      await this.runtime.exclusive(id, async () => {
        need('files');
        const grants = await this.fileGrants(id); grants[token] = await fs.realpath(result.filePaths[0]!);
        need('files'); await atomicJson(join(this.store.directory(id), 'file-grants.json'), grants);
      });
      return { token, name: result.filePaths[0].split(sep).at(-1) };
    }
    if (input.method.startsWith('files.')) {
      need('files');
      const root = args.root === 'data' ? this.store.dataPath(id) : (await this.fileGrants(id))[String(args.root)];
      if (!root) throw new Error('Select a folder first');
      const rel = relativePath(args.path ?? '.');
      if (input.method === 'files.read') return fs.readFile(await containedFile(root, rel), 'utf8');
      if (input.method === 'files.list') return (await fs.readdir(await containedFile(root, rel), { withFileTypes: true })).map(e => ({ name: e.name, directory: e.isDirectory() }));
      if (input.method === 'files.write') {
        const leaf = rel.split('/').at(-1);
        if (!leaf || leaf === '.') throw new Error('Writing requires a file path');
        const parent = await containedFile(root, dirname(rel).split(sep).join('/'));
        const target = join(parent, leaf);
        try { if (!(await fs.lstat(target)).isFile()) throw new Error('Cannot write through a symlink or replace a directory'); } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
        if (typeof args.text !== 'string') throw new Error('Expected text');
        const temp = `${target}.${randomUUID()}.tmp`;
        try { need('files'); await fs.writeFile(temp, args.text, { flag: 'wx', mode: 0o600 }); need('files'); await fs.rename(temp, target); }
        finally { await fs.rm(temp, { force: true }); }
        return;
      }
    }
    throw new Error('Unknown application API');
  }
  private async fileGrants(id: string): Promise<Record<string, string>> { try { return JSON.parse(await fs.readFile(join(this.store.directory(id), 'file-grants.json'), 'utf8')); } catch { return {}; } }
  hasRemoteAccess(): boolean { return this.remoteOpening.size > 0 || !!this.remoteGateway?.hasAccess(); }
  async hasBackgroundApps(): Promise<boolean> {
    if (this.hasRemoteAccess()) return true;
    const rows = await this.list();
    return this.hasRemoteAccess() || rows.some(row => (row.background && ['running', 'installing', 'building', 'starting'].includes(row.phase))
      || (row.agent?.enabled && row.agent.background && this.agentBroker?.hasActiveApp(row.id)));
  }
  private retainRemoteRun(id: string): boolean {
    const runId = this.runtime.invocationId(id);
    if (!this.remoteOpening.has(id) && !(runId && this.remoteGateway?.hasRun(id, runId))) return false;
    // Reconcile when the last opening fails too, even for a run that originally
    // belonged to an independent background grant rather than a remote launch.
    if (runId) this.remoteManagedRuns.set(id, runId);
    return true;
  }
  async dispose(): Promise<void> {
    this.shuttingDown = true;
    const mcpCleanup = Promise.allSettled([this.mcp.dispose(), this.mcpGateway.dispose()]);
    this.unsubscribeAgentSettlement?.(); this.unsubscribeAgentSettlement = undefined;
    this.invalidateRemoteOpenings();
    await this.remoteGateway?.dispose();
    this.remoteRtc?.dispose();
    for (const pending of this.agentQuestions.values()) pending.resolve(null);
    await this.agentBroker?.dispose();
    for (const operation of this.opening.values()) operation.controller.abort();
    for (const waiting of this.waitingOpenings.values()) for (const controller of waiting) controller.abort();
    if (this.changeTimer) clearTimeout(this.changeTimer);
    ipcMain.removeHandler(CH_LOCAL_APPS); ipcMain.removeHandler(CH_LOCAL_APP_NATIVE);
    this.server?.closeAllConnections(); await new Promise<void>(resolve => this.server ? this.server.close(() => resolve()) : resolve());
    await this.runtime.dispose();
    await this.services.dispose();
    await this.options.core.dispose();
    await Promise.allSettled([...this.opening.values()].map(operation => operation.promise));
    for (const window of this.windows.values()) window.destroy();
    for (const id of this.configured) {
      const partition = session.fromPartition(`persist:local-app-${id}`);
      partition.protocol.unhandle(LOCAL_APP_SCHEME);
      partition.setPermissionCheckHandler(null);
      partition.setPermissionRequestHandler(null);
      partition.flushStorageData();
    }
    this.configured.clear(); this.launchGrants.clear();
    const path = join(this.options.configHome, 'desktop-local-app-host.json');
    try { if (JSON.parse(await fs.readFile(path, 'utf8')).token === this.token) await fs.rm(path); } catch { /* Already removed. */ }
    const mcpErrors = (await mcpCleanup).filter((result): result is PromiseRejectedResult => result.status === 'rejected').map(result => result.reason);
    if (mcpErrors.length) throw new AggregateError(mcpErrors, 'Local App MCP shutdown needs retry');
  }
}
