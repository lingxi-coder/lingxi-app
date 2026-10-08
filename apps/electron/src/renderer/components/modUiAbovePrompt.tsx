import * as React from 'react';
import {
  createContext,
  memo,
  useCallback,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import type {
  NativeUiComponent,
  NativeUiControlRequest,
  NativeUiControlResponseFor,
  NativeUiParentControlRequest,
  NativeUiRenderResponseDto,
  NativeUiSurfaceDto,
  NativeUiViewportDto,
  UiClientFrameDto,
  UiClientFrameEventDto,
  UiClientOperation,
  UiClientOperationResponseFor,
  UiControlCallResultDto,
  UiInvalidateEventDto,
  UiJsonValue,
} from '@lingxi/bridge-client';
import { ModUiClientTree, parseModUiClientTree, type ModUiClientIdentity } from './modUiClientTree';
import {
  ModUiParentTree,
  collectModUiClientElements,
  parseModUiParentTree,
  type ModUiParentClientDescriptor,
  type ModUiParentEngineFallback,
} from './modUiParentTree';
export { collectModUiClientElements } from './modUiParentTree';

export interface ModUiClientHostApi {
  control<T extends NativeUiControlRequest>(
    sessionId: string,
    request: T,
  ): Promise<UiControlCallResultDto<NativeUiControlResponseFor<T>>>;
  operation<T extends UiClientOperation>(
    sessionId: string,
    operation: T,
  ): Promise<UiClientOperationResponseFor<T>>;
  onFrame(listener: (event: UiClientFrameEventDto) => void): () => void;
  onInvalidate(listener: (event: UiInvalidateEventDto) => void): () => void;
}

export interface ModUiAbovePromptProps {
  sessionId: string;
  host?: ModUiClientHostApi;
  enabled?: boolean;
  engineFallback?: ModUiParentEngineFallback;
  children?: ReactNode;
}

export interface ModUiParentSiteProps {
  sessionId: string;
  surface: 'desktop';
  component: NativeUiComponent;
  instanceId: string;
  props: Record<string, UiJsonValue>;
  viewport?: NativeUiViewportDto;
  host?: ModUiClientHostApi;
  enabled?: boolean;
  engineFallback?: ModUiParentEngineFallback;
  children?: ReactNode;
}

const ModUiSurfaceClientIdContext = createContext<string | null>(null);

export function ModUiSurfaceClientIdProvider({ clientId, children }: { clientId: string; children: ReactNode }) {
  return <ModUiSurfaceClientIdContext.Provider value={clientId}>{children}</ModUiSurfaceClientIdContext.Provider>;
}

type ClientElementDescriptor = ModUiParentClientDescriptor;

export function isClientControlAddressable(client: ModUiClientIdentity): boolean {
  return client.plugin.length <= 256 && client.key.length <= 256 && client.module.length <= 256;
}

interface ClientFrameSnapshot extends Omit<UiClientFrameDto, 'runtimeId'> {}

interface WorkerFaultFrameSnapshot {
  renderRevision: number;
  fault: { phase: 'load' | 'render' | 'run'; reason: string; source: 'worker' };
}

type ClientRuntimeFrame = ClientFrameSnapshot | WorkerFaultFrameSnapshot;

interface RenderSite {
  sessionId: string;
  surface: 'desktop';
  component: NativeUiComponent;
  instanceId: string;
  renderRevision: number;
  clientRuntimeEpochs: Readonly<Record<string, number>>;
  clientStateToken: string | null;
  committedRevision: number | null;
  drawnClients: readonly ClientElementDescriptor[];
}

interface ModUiClientParentContext {
  host: ModUiClientHostApi;
  site: RenderSite;
  registerFrame(runtimeId: string, revision: number, listener: (frame: ClientRuntimeFrame) => void): () => void;
  reportFault(client: ModUiClientIdentity, phase: 'load' | 'render' | 'run', reason: string): void;
}

interface BufferedClientFrame {
  revision: number;
  frame: ClientRuntimeFrame;
}

function runtimeEpochForPlugin(epochs: Readonly<Record<string, number>> | undefined, plugin: string): number | null {
  const epoch = epochs?.[plugin];
  return typeof epoch === 'number' && Number.isSafeInteger(epoch) && epoch > 0 ? epoch : null;
}

const MAX_BUFFERED_CLIENT_FRAMES = 128;

function unmountParentDraw(host: ModUiClientHostApi, site: Pick<RenderSite, 'surface' | 'component' | 'instanceId'>, sessionId: string, revision: number | null): void {
  if (revision === null) return;
  const request = {
    type: 'draw_unmount',
    surface: site.surface,
    component: site.component,
    instance_id: site.instanceId,
    render_revision: revision,
  } satisfies UiClientOperation;
  void host.operation(sessionId, request).catch(() => undefined);
}

const ParentContext = createContext<ModUiClientParentContext | null>(null);
const ABOVE_PROMPT_COMPONENT: 'AbovePrompt' = 'AbovePrompt';
const CLIENT_TREE_FONT = 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace';
const CLIENT_TREE_FONT_SIZE = 13;
const CLIENT_TREE_LINE_HEIGHT = 1.4;
const MAX_FAULT_REASON_UTF16 = 200;

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}

/** Convert measured pixels into Native VM character rows/columns. */
export function gridSizeFromPixels(width: number, height: number, cellWidth: number, lineHeight: number): { columns: number; rows: number } | null {
  if (![width, height, cellWidth, lineHeight].every(Number.isFinite)
    || width <= 0 || height <= 0 || cellWidth <= 0 || lineHeight <= 0) return null;
  return {
    columns: Math.max(1, Math.floor(width / cellWidth)),
    rows: Math.max(1, Math.floor(height / lineHeight)),
  };
}

function measureGrid(element: HTMLElement, allowParentViewport: boolean): { columns: number; rows: number } | null {
  if (typeof window === 'undefined' || typeof document === 'undefined') return null;
  const ownRect = element.getBoundingClientRect();
  let width = ownRect.width;
  let height = ownRect.height;
  if (allowParentViewport && (width <= 0 || height <= 0)) {
    const main = element.closest('.desktop-main') as HTMLElement | null;
    const parentRect = (main ?? document.documentElement).getBoundingClientRect();
    if (width <= 0) width = parentRect.width;
    if (height <= 0) height = parentRect.height;
  }
  const style = window.getComputedStyle(element);
  const fontSize = allowParentViewport ? CLIENT_TREE_FONT_SIZE : Number.parseFloat(style.fontSize);
  if (!Number.isFinite(fontSize) || fontSize <= 0) return null;
  const parsedLineHeight = Number.parseFloat(style.lineHeight);
  const lineHeight = allowParentViewport
    ? fontSize * CLIENT_TREE_LINE_HEIGHT
    : Number.isFinite(parsedLineHeight) && parsedLineHeight > 0 ? parsedLineHeight : fontSize * CLIENT_TREE_LINE_HEIGHT;
  const canvas = document.createElement('canvas');
  const context = canvas.getContext('2d');
  if (!context) return null;
  context.font = allowParentViewport
    ? `${fontSize}px ${CLIENT_TREE_FONT}`
    : style.font || `${fontSize}px ${style.fontFamily || CLIENT_TREE_FONT}`;
  const cellWidth = context.measureText('M').width;
  return gridSizeFromPixels(width, height, cellWidth, lineHeight);
}

export function invalidationTargetsRenderSite(
  event: UiInvalidateEventDto,
  sessionId: string,
  instanceId: string,
  component: NativeUiComponent = ABOVE_PROMPT_COMPONENT,
  surface: NativeUiSurfaceDto = 'desktop',
): boolean {
  if (event.sessionId !== sessionId) return false;
  if (event.instances === undefined) return true;
  return event.instances.some((site) => site.surface === surface
    && site.component === component && site.instance_id === instanceId);
}

/** Resolve a hook-adjusted inner element key against the current VM frame. */
export function heldHandleForElement(tree: UiJsonValue, element: string, plugin?: string): number | null {
  let parsed;
  try {
    parsed = collectInteractiveNodes(parseModUiClientTree(tree, plugin) as unknown as UiJsonValue);
  } catch {
    return null;
  }
  const matches = parsed.filter((entry) => entry.element === element);
  return matches.length === 1 ? matches[0].handle : null;
}

function collectInteractiveNodes(tree: UiJsonValue): Array<{ element: string; handle: number }> {
  const result: Array<{ element: string; handle: number }> = [];
  const visit = (value: UiJsonValue): void => {
    if (Array.isArray(value)) {
      for (const child of value) visit(child);
      return;
    }
    if (!isRecord(value)) return;
    const type = value.type;
    const props = value.props;
    const press = value.press;
    if ((type === 'Button' || type === 'Input' || type === 'Select' || type === 'Markdown')
      && isRecord(props) && isRecord(press) && typeof props.key === 'string'
      && typeof press.handle === 'number' && Number.isSafeInteger(press.handle) && press.handle > 0) {
      result.push({ element: props.key, handle: press.handle });
    }
    if (Array.isArray(value.children)) {
      for (const child of value.children) {
        if (child === null || typeof child === 'boolean' || typeof child === 'number' || typeof child === 'string' || Array.isArray(child) || isRecord(child)) {
          visit(child as UiJsonValue);
        }
      }
    }
  };
  visit(tree);
  return result;
}

function truncateUtf16(value: string, limit: number): string {
  let result = '';
  let units = 0;
  for (const character of value) {
    const nextUnits = character.length;
    if (units + nextUnits > limit) break;
    result += character;
    units += nextUnits;
  }
  return result;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isFrame(value: unknown): value is UiClientFrameDto {
  return isRecord(value) && typeof value.runtimeId === 'string'
    && typeof value.renderRevision === 'number' && Number.isFinite(value.renderRevision)
    && Number.isSafeInteger(value.frameSequence) && Number(value.frameSequence) > 0
    && typeof value.hasPointerListener === 'boolean' && typeof value.hasKeyListener === 'boolean'
    && 'tree' in value;
}

function isWorkerFaultFrame(value: unknown): value is WorkerFaultFrameSnapshot {
  if (!isRecord(value) || typeof value.renderRevision !== 'number' || !Number.isSafeInteger(value.renderRevision)
    || !isRecord(value.fault)) return false;
  return (value.fault.phase === 'load' || value.fault.phase === 'render' || value.fault.phase === 'run')
    && typeof value.fault.reason === 'string' && value.fault.source === 'worker'
    && !('frameSequence' in value) && !('tree' in value);
}

function isStaleOperation(value: unknown): value is { handled: false; renderRevision: number } {
  return isRecord(value) && value.handled === false
    && typeof value.renderRevision === 'number' && Number.isFinite(value.renderRevision)
    && !('fault' in value);
}

function isWorkerFault(value: unknown): value is {
  handled: false;
  renderRevision: number;
  runtimeId?: string;
  fault: { phase: 'load' | 'render' | 'run'; reason: string; source: 'worker' };
} {
  if (!isRecord(value) || value.handled !== false
    || typeof value.renderRevision !== 'number' || !Number.isFinite(value.renderRevision)
    || !isRecord(value.fault)) return false;
  return (value.runtimeId === undefined || typeof value.runtimeId === 'string')
    && (value.fault.phase === 'load' || value.fault.phase === 'render' || value.fault.phase === 'run')
    && typeof value.fault.reason === 'string' && value.fault.source === 'worker';
}

function clientElementStyle(descriptor: ClientElementDescriptor): React.CSSProperties {
  return {
    width: descriptor.width,
    height: descriptor.height,
    flexGrow: descriptor.flexGrow,
    minWidth: 0,
    minHeight: `${CLIENT_TREE_FONT_SIZE * CLIENT_TREE_LINE_HEIGHT}px`,
    overflow: 'auto',
    fontFamily: CLIENT_TREE_FONT,
    fontSize: CLIENT_TREE_FONT_SIZE,
    lineHeight: CLIENT_TREE_LINE_HEIGHT,
  };
}

const EMPTY_PARENT_PROPS: Record<string, UiJsonValue> = {};

export const ModUiAbovePrompt = memo(function ModUiAbovePrompt({ sessionId, host, enabled = true, engineFallback, children }: ModUiAbovePromptProps) {
  const generatedId = useId();
  return <ModUiParentSite
    sessionId={sessionId}
    surface="desktop"
    component={ABOVE_PROMPT_COMPONENT}
    instanceId={`above-prompt-${generatedId}`}
    props={EMPTY_PARENT_PROPS}
    host={host}
    enabled={enabled}
    engineFallback={engineFallback}
  >{children}</ModUiParentSite>;
});

export const ModUiParentSite = memo(function ModUiParentSite({
  sessionId,
  surface,
  component,
  instanceId,
  props,
  viewport,
  host,
  enabled = true,
  engineFallback,
  children,
}: ModUiParentSiteProps) {
  const clientId = useContext(ModUiSurfaceClientIdContext);
  const resolvedHost = host ?? (typeof window === 'undefined' ? undefined : window.lingxi?.modUi);
  if (!enabled || !sessionId || !resolvedHost) return <>{children}</>;
  return React.createElement(ModUiParentSiteRuntime, {
    key: `${sessionId}\u0000${surface}\u0000${component}\u0000${instanceId}`,
    host: resolvedHost,
    sessionId,
    surface,
    component,
    instanceId,
    props,
    clientId,
    viewport,
    engineFallback,
    children,
  });
});

function ModUiParentSiteRuntime({
  host,
  sessionId,
  surface,
  component,
  instanceId,
  props,
  clientId,
  viewport,
  engineFallback,
  children,
}: Required<Pick<ModUiParentSiteProps, 'host' | 'sessionId' | 'surface' | 'component' | 'instanceId' | 'props'>>
  & Pick<ModUiParentSiteProps, 'viewport' | 'engineFallback' | 'children'>
  & { clientId: string | null }) {
  const rootRef = useRef<HTMLDivElement>(null);
  const frameListeners = useRef(new Map<string, { revision: number; listener: (frame: ClientRuntimeFrame) => void }>());
  const bufferedFrames = useRef(new Map<string, BufferedClientFrame>());
  const requestSequence = useRef(0);
  const latestViewport = useRef<NativeUiViewportDto | null>(null);
  const requestRenderRef = useRef<() => void>(() => undefined);
  const seenInvalidations = useRef(new Set<string>());
  const drawnRef = useRef<{ sessionId: string; revision: number } | null>(null);
  const latestRenderRevision = useRef<number | null>(null);
  const [parentFrame, setParentFrame] = useState<{
    sessionId: string;
    revision: number;
    response: NativeUiRenderResponseDto;
    clientRuntimeEpochs: Readonly<Record<string, number>>;
    clientStateToken: string | null;
  } | null>(null);
  const [drawn, setDrawn] = useState<{ sessionId: string; revision: number; clientsKey: string } | null>(null);
  const [hostError, setHostError] = useState<string | null>(null);

  const registerFrame = useCallback((runtimeId: string, revision: number, listener: (frame: ClientRuntimeFrame) => void) => {
    const registration = { revision, listener };
    frameListeners.current.set(runtimeId, registration);
    const buffered = bufferedFrames.current.get(runtimeId);
    if (buffered?.revision === revision && latestRenderRevision.current === revision) {
      bufferedFrames.current.delete(runtimeId);
      listener(buffered.frame);
    }
    return () => {
      if (frameListeners.current.get(runtimeId) === registration) frameListeners.current.delete(runtimeId);
    };
  }, []);

  const requestRender = useCallback(() => {
    const viewport = latestViewport.current;
    if (!viewport) return;
    const sequence = ++requestSequence.current;
    const request = {
      subtype: 'ui_render',
      surface,
      component,
      instance_id: instanceId,
      props,
      ...(clientId === null ? {} : { client_id: clientId }),
      viewport,
    } satisfies Extract<NativeUiControlRequest, { subtype: 'ui_render' }>;
    void host.control(sessionId, request).then((result) => {
      const revision = result.metadata?.renderRevision;
      if (sequence !== requestSequence.current) {
        if (revision !== undefined && Number.isFinite(revision)) {
          // A late valid response still owns Host-side render state. Send its
          // exact revision so Host generation checks can fence the cleanup.
          unmountParentDraw(host, { surface, component, instanceId }, sessionId, revision);
        }
        return;
      }
      if (revision === undefined || !Number.isFinite(revision)) {
        if (result.response.tree === null) {
          const previousRevision = latestRenderRevision.current ?? drawnRef.current?.revision ?? null;
          latestRenderRevision.current = null;
          bufferedFrames.current.clear();
          drawnRef.current = null;
          setDrawn(null);
          setHostError(null);
          setParentFrame(null);
          unmountParentDraw(host, { surface, component, instanceId }, sessionId, previousRevision);
          return;
        }
        setHostError('The UI host returned no render revision.');
        return;
      }
      if (latestRenderRevision.current !== revision) {
        const bufferedFaults = [...bufferedFrames.current.entries()]
          .filter(([, buffered]) => isWorkerFaultFrame(buffered.frame));
        bufferedFrames.current.clear();
        for (const [runtimeId, buffered] of bufferedFaults) {
          bufferedFrames.current.set(runtimeId, { revision, frame: buffered.frame });
        }
      }
      latestRenderRevision.current = revision;
      setHostError(null);
      setParentFrame({
        sessionId,
        revision,
        response: result.response,
        clientRuntimeEpochs: result.metadata?.clientRuntimeEpochs ?? {},
        clientStateToken: result.metadata?.clientStateToken ?? null,
      });
    }).catch((error: unknown) => {
      if (sequence !== requestSequence.current) return;
      setHostError(errorMessage(error));
    });
  }, [clientId, component, host, instanceId, props, sessionId, surface]);
  requestRenderRef.current = requestRender;

  useEffect(() => {
    if (latestViewport.current) requestRenderRef.current();
  }, [props]);

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    let active = true;
    let lastGrid = '';
    let scheduled = 0;
    const updateViewport = () => {
      if (!active) return;
      const grid = measureGrid(root, true);
      const nextViewport = viewport ?? grid;
      if (!nextViewport) return;
      const isFullscreen = 'isFullscreen' in nextViewport ? nextViewport.isFullscreen : undefined;
      const key = `${nextViewport.columns}:${nextViewport.rows}:${isFullscreen ?? ''}`;
      if (key === lastGrid) return;
      lastGrid = key;
      latestViewport.current = nextViewport;
      requestRenderRef.current();
    };
    const observer = new ResizeObserver(() => {
      window.cancelAnimationFrame(scheduled);
      scheduled = window.requestAnimationFrame(updateViewport);
    });
    observer.observe(root);
    const viewportRoot = root.closest('.desktop-main') as HTMLElement | null;
    if (viewportRoot && viewportRoot !== root) observer.observe(viewportRoot);
    updateViewport();
    const unsubscribeFrames = host.onFrame((event) => {
      if (!active || event.sessionId !== sessionId) return;
      const expectedRevision = latestRenderRevision.current;
      if (expectedRevision === null) return;
      const registered = frameListeners.current.get(event.runtimeId);
      if (isWorkerFaultFrame(event.frame)) {
        if (registered) {
          registered.listener(event.frame);
          return;
        }
        const bufferedFault = bufferedFrames.current.get(event.runtimeId);
        if (isWorkerFaultFrame(bufferedFault?.frame)) return;
        bufferedFrames.current.delete(event.runtimeId);
        bufferedFrames.current.set(event.runtimeId, { revision: expectedRevision, frame: event.frame });
        while (bufferedFrames.current.size > MAX_BUFFERED_CLIENT_FRAMES) {
          const oldest = bufferedFrames.current.keys().next().value;
          if (oldest === undefined) break;
          bufferedFrames.current.delete(oldest);
        }
        return;
      }
      if (event.frame.renderRevision !== expectedRevision) return;
      if (registered?.revision === expectedRevision) {
        registered.listener(event.frame);
        return;
      }
      // Mount can drain a queued schedule/post before its promise resolves and
      // before React registers the runtime listener. Keep only the newest frame
      // for the active parent revision; the session map is capped defensively.
      const buffered = bufferedFrames.current.get(event.runtimeId);
      if (isWorkerFaultFrame(buffered?.frame)) return;
      if (buffered?.revision === expectedRevision && !isWorkerFaultFrame(buffered.frame)
        && buffered.frame.frameSequence >= event.frame.frameSequence) return;
      bufferedFrames.current.delete(event.runtimeId);
      bufferedFrames.current.set(event.runtimeId, { revision: expectedRevision, frame: event.frame });
      while (bufferedFrames.current.size > MAX_BUFFERED_CLIENT_FRAMES) {
        const oldest = bufferedFrames.current.keys().next().value;
        if (oldest === undefined) break;
        bufferedFrames.current.delete(oldest);
      }
    });
    const unsubscribeInvalidation = host.onInvalidate((event) => {
      if (!active || !invalidationTargetsRenderSite(event, sessionId, instanceId, component, surface)) return;
      if (seenInvalidations.current.has(event.uuid)) return;
      seenInvalidations.current.add(event.uuid);
      if (seenInvalidations.current.size > 256) seenInvalidations.current.clear();
      requestRenderRef.current();
    });
    return () => {
      active = false;
      observer.disconnect();
      window.cancelAnimationFrame(scheduled);
      unsubscribeFrames();
      unsubscribeInvalidation();
      requestSequence.current += 1;
      bufferedFrames.current.clear();
      frameListeners.current.clear();
      unmountParentDraw(host, { surface, component, instanceId }, sessionId, latestRenderRevision.current ?? drawnRef.current?.revision ?? null);
    };
  }, [component, host, instanceId, sessionId, surface, viewport]);

  const parentTree = useMemo(() => {
    if (!parentFrame || parentFrame.sessionId !== sessionId) return {
      tree: null as UiJsonValue | null,
      clients: [] as ClientElementDescriptor[],
      error: null as string | null,
    };
    try {
      const tree = parseModUiParentTree(parentFrame.response.tree);
      return { tree, clients: collectModUiClientElements(tree ?? null), error: null as string | null };
    } catch (error) {
      return { tree: { type: 'engine', ref: 0 } as UiJsonValue, clients: [] as ClientElementDescriptor[], error: errorMessage(error) };
    }
  }, [parentFrame, sessionId]);
  const drawnClients = hostError || parentTree.error ? [] : parentTree.clients;
  const clientsKey = useMemo(() => drawnClients.map((client) => {
    const epoch = runtimeEpochForPlugin(parentFrame?.clientRuntimeEpochs, client.plugin);
    return `${client.plugin}\u0000${client.key}\u0000${client.module}\u0000${epoch ?? 'missing-epoch'}`;
  }).join('\u0001'), [drawnClients, parentFrame?.clientRuntimeEpochs]);
  const renderRevision = parentFrame?.sessionId === sessionId ? parentFrame.revision : null;

  useLayoutEffect(() => {
    if (renderRevision === null) return;
    let active = true;
    const request = {
      type: 'draw_commit',
      surface,
      component,
      instance_id: instanceId,
      render_revision: renderRevision,
      clients: drawnClients.map(({ plugin, key, module }) => ({ plugin, key, module })),
    } satisfies UiClientOperation;
    void host.operation(sessionId, request).then((result) => {
      if (!active || !('handled' in result) || !result.handled || result.renderRevision !== renderRevision) return;
      drawnRef.current = { sessionId, revision: renderRevision };
      setDrawn({ sessionId, revision: renderRevision, clientsKey });
    }).catch((error: unknown) => {
      if (active) setHostError(errorMessage(error));
    });
    return () => { active = false; };
  }, [clientsKey, component, drawnClients, host, instanceId, renderRevision, sessionId, surface]);

  const committedRevision = drawn?.sessionId === sessionId && drawn.revision === renderRevision && drawn.clientsKey === clientsKey
    ? drawn.revision
    : null;
  const site = useMemo<RenderSite | null>(() => renderRevision === null ? null : ({
    sessionId,
    surface,
    component,
    instanceId,
    renderRevision,
    clientRuntimeEpochs: parentFrame?.clientRuntimeEpochs ?? {},
    clientStateToken: parentFrame?.clientStateToken ?? null,
    committedRevision,
    drawnClients,
  }), [committedRevision, component, drawnClients, instanceId, parentFrame?.clientRuntimeEpochs,
    parentFrame?.clientStateToken, renderRevision, sessionId, surface]);

  const reportFault = useCallback((client: ModUiClientIdentity, phase: 'load' | 'render' | 'run', reason: string) => {
    if (!site || site.committedRevision !== site.renderRevision) return;
    const request = {
      subtype: 'ui_client_fault',
      plugin: client.plugin,
      component: site.component,
      instance_id: site.instanceId,
      client: client.key,
      module: client.module,
      phase,
      reason: truncateUtf16(reason || 'Client surface failed', MAX_FAULT_REASON_UTF16),
    } satisfies Extract<NativeUiControlRequest, { subtype: 'ui_client_fault' }>;
    void host.control(sessionId, request).catch(() => undefined);
  }, [host, sessionId, site]);

  const context = useMemo<ModUiClientParentContext | null>(() => site ? {
    host,
    site,
    registerFrame,
    reportFault,
  } : null, [host, registerFrame, reportFault, site]);

  const isAbovePrompt = component === ABOVE_PROMPT_COMPONENT;
  const siteRootStyle: React.CSSProperties = isAbovePrompt
    ? { width: '100%', flexShrink: 0, display: 'flex', justifyContent: 'center', minWidth: 0 }
    : { width: '100%', flexShrink: 0, minWidth: 0 };
  const siteContentStyle: React.CSSProperties = isAbovePrompt
    ? { width: '100%', maxWidth: 'var(--conversation-width, 860px)', minWidth: 0 }
    : { width: '100%', minWidth: 0 };
  return <div ref={rootRef} className={isAbovePrompt ? 'mod-ui-above-prompt' : 'mod-ui-parent-site'} data-component={component} style={siteRootStyle}>
    <div className={isAbovePrompt ? 'mod-ui-above-prompt-content' : 'mod-ui-parent-site-content'} style={siteContentStyle}>
      {hostError || parentFrame?.sessionId !== sessionId || !context
        ? children
        : <ParentContext.Provider value={context}>
            <ModUiParentTree
              tree={parentTree.tree}
              site={{ surface, component, instanceId }}
              requestProps={props}
              responseProps={parentFrame.response.props}
              fallback={children}
              engineFallback={engineFallback}
              onParentControl={(request: NativeUiParentControlRequest) => host.control(sessionId, clientId === null
                ? request
                : { ...request, client_id: clientId })}
              renderClient={(descriptor, reactKey) => {
                const epoch = runtimeEpochForPlugin(parentFrame.clientRuntimeEpochs, descriptor.plugin);
                return <ClientModuleSurface key={`${reactKey}\u0000${descriptor.plugin}\u0000${descriptor.key}\u0000${descriptor.module}\u0000${epoch ?? 'missing-epoch'}`} descriptor={descriptor} />;
              }}
            />
          </ParentContext.Provider>}
    </div>
  </div>;
}

function ClientModuleSurface({ descriptor }: { descriptor: ClientElementDescriptor }) {
  const context = useContext(ParentContext);
  const elementRef = useRef<HTMLDivElement>(null);
  const runtimeIdRef = useRef<string | null>(null);
  const currentRevisionRef = useRef<number | null>(context?.site.renderRevision ?? null);
  const mountedRevisionRef = useRef<number | null>(null);
  const operationGeneration = useRef(0);
  const mountPending = useRef(false);
  const mountOwnerGeneration = useRef<number | null>(null);
  const refreshAfterMount = useRef(false);
  const operationTail = useRef<Promise<void>>(Promise.resolve());
  const [runtimeId, setRuntimeId] = useState<string | null>(null);
  const [frame, setFrame] = useState<ClientFrameSnapshot | null>(null);
  const frameRef = useRef<ClientFrameSnapshot | null>(null);
  const latestFrameSequenceRef = useRef(0);
  const failedRuntimeEpochRef = useRef<number | null>(null);
  const failedRuntimeIdRef = useRef<string | null>(null);
  const failedRuntimeStateTokenRef = useRef<string | null>(null);
  const [fault, setFault] = useState<{
    phase: 'load' | 'render' | 'run';
    message: string;
    revision: number;
    surfaceHeight: number | null;
    source: 'adapter' | 'worker';
    runtimeEpoch: number | null;
    clientStateToken: string | null;
  } | null>(null);
  const [measurementRevision, setMeasurementRevision] = useState(0);

  const site = context?.site ?? null;
  const runtimeEpoch = runtimeEpochForPlugin(site?.clientRuntimeEpochs, descriptor.plugin);
  const runtimeEpochRef = useRef<number | null>(runtimeEpoch);
  runtimeEpochRef.current = runtimeEpoch;
  const clientStateToken = site?.clientStateToken ?? null;
  const clientStateTokenRef = useRef<string | null>(clientStateToken);
  clientStateTokenRef.current = clientStateToken;
  const previousClientStateTokenRef = useRef<string | null>(clientStateToken);
  const runtimeEpochAvailable = runtimeEpoch !== null;
  currentRevisionRef.current = site?.renderRevision ?? null;
  frameRef.current = frame;
  const lastHostRef = useRef<ModUiClientHostApi | null>(context?.host ?? null);
  const lastSessionIdRef = useRef<string | null>(context?.site.sessionId ?? null);
  if (context) {
    lastHostRef.current = context.host;
    lastSessionIdRef.current = context.site.sessionId;
  }

  const captureFault = useCallback((
    phase: 'load' | 'render' | 'run',
    message: string,
    revision: number,
    source: 'adapter' | 'worker',
    failedEpoch: number | null,
  ) => {
    const height = elementRef.current?.getBoundingClientRect().height;
    setFault({
      phase,
      message,
      revision,
      source,
      runtimeEpoch: failedEpoch,
      clientStateToken: clientStateTokenRef.current,
      // Keep the placeholder from collapsing the observed Client viewport.
      // This is layout stability only; worker lifecycle recovery is fenced by
      // the Host's plugin runtime epoch below.
      surfaceHeight: height !== undefined && Number.isFinite(height) && height > 0 ? height : null,
    });
  }, []);
  const showFault = useCallback((phase: 'load' | 'render' | 'run', message: string, revision: number) => {
    captureFault(phase, message, revision, 'adapter', null);
  }, [captureFault]);

  const acceptFrame = useCallback((
    nextFrame: ClientFrameSnapshot,
    expectedRevision: number | null,
    sourceRuntimeId?: string,
  ) => {
    if (expectedRevision === null || nextFrame.renderRevision !== expectedRevision
      || nextFrame.frameSequence <= latestFrameSequenceRef.current) return false;
    if ((runtimeEpochRef.current !== null && failedRuntimeEpochRef.current === runtimeEpochRef.current)
      || (sourceRuntimeId !== undefined && failedRuntimeIdRef.current === sourceRuntimeId)) return false;
    latestFrameSequenceRef.current = nextFrame.frameSequence;
    setFrame(nextFrame);
    // A Client run fault belongs to this drawn plugin/module/key identity.
    // Passive frames and same-identity prop updates don't prove the runtime
    // state changed; only the Host's state token or a true remount can reset it.
    setFault((currentFault) => currentFault?.source === 'adapter' && currentFault.phase === 'run'
      && (currentFault.clientStateToken === null || clientStateTokenRef.current === null
        || currentFault.clientStateToken === clientStateTokenRef.current)
      ? currentFault
      : null);
    return true;
  }, []);

  const showWorkerFault = useCallback((phase: 'load' | 'render' | 'run', message: string, _revision: number, runtimeId?: string) => {
    const currentRevision = currentRevisionRef.current;
    if (currentRevision === null) return;
    if (runtimeId !== undefined && runtimeIdRef.current !== null && runtimeIdRef.current !== runtimeId) return;
    failedRuntimeEpochRef.current = runtimeEpochRef.current;
    if (runtimeId !== undefined) failedRuntimeIdRef.current = runtimeId;
    failedRuntimeStateTokenRef.current = clientStateTokenRef.current;
    captureFault(phase, message, currentRevision, 'worker', runtimeEpochRef.current);
  }, [captureFault]);

  // Native invalidates a Client failure only when its authoritative state
  // version changes. Parent render revisions and prop updates are not state
  // versions, so unknown or unchanged tokens leave the failure in place.
  useEffect(() => {
    const previousToken = previousClientStateTokenRef.current;
    previousClientStateTokenRef.current = clientStateToken;
    if (previousToken === null || clientStateToken === null || previousToken === clientStateToken) return;
    if (failedRuntimeStateTokenRef.current !== null
      && failedRuntimeStateTokenRef.current !== clientStateToken) {
      failedRuntimeEpochRef.current = null;
      failedRuntimeIdRef.current = null;
      failedRuntimeStateTokenRef.current = null;
    }
    setFault((currentFault) => currentFault?.clientStateToken !== null
      && currentFault?.clientStateToken !== undefined
      && currentFault.clientStateToken !== clientStateToken ? null : currentFault);
  }, [clientStateToken]);

  const queueOperation = useCallback(<T extends UiClientOperation>(operation: T): Promise<UiClientOperationResponseFor<T>> => {
    if (!context) return Promise.reject(new Error('Client surface host is unavailable'));
    const run = operationTail.current.catch(() => undefined).then(() => context.host.operation(context.site.sessionId, operation));
    operationTail.current = run.then(() => undefined, () => undefined);
    return run;
  }, [context]);

  const reportCurrentFault = useCallback((phase: 'load' | 'render' | 'run', message: string, revision: number) => {
    if (!context || currentRevisionRef.current !== revision || context.site.committedRevision !== revision) return;
    showFault(phase, message, revision);
    context.reportFault(descriptor, phase, message);
  }, [context, descriptor, showFault]);

  useEffect(() => {
    if (!context || !runtimeId || !runtimeEpochAvailable) return;
    const expectedRevision = currentRevisionRef.current;
    if (expectedRevision === null) return;
    const unregister = context.registerFrame(runtimeId, expectedRevision, (nextFrame) => {
      if (isWorkerFaultFrame(nextFrame)) {
        showWorkerFault(nextFrame.fault.phase, nextFrame.fault.reason, nextFrame.renderRevision, runtimeId);
        return;
      }
      acceptFrame(nextFrame, currentRevisionRef.current, runtimeId);
    });
    return unregister;
  }, [acceptFrame, context, runtimeEpochAvailable, runtimeId, showWorkerFault]);

  useEffect(() => {
    const element = elementRef.current;
    if (!element) return;
    let active = true;
    let lastKey = '';
    const observer = new ResizeObserver(() => {
      if (!context) return;
      if (!runtimeEpochAvailable || failedRuntimeEpochRef.current === runtimeEpochRef.current) return;
      const revision = currentRevisionRef.current;
      if (revision === null) return;
      const grid = measureGrid(element, false);
      if (!grid) return;
      const key = `${grid.columns}:${grid.rows}`;
      if (key === lastKey) return;
      lastKey = key;
      const activeRuntimeId = runtimeIdRef.current;
      if (!activeRuntimeId) {
        if (!mountPending.current) setMeasurementRevision((value) => value + 1);
        return;
      }
      const request = {
        type: 'resize',
        runtimeId: activeRuntimeId,
        render_revision: revision,
        columns: grid.columns,
        rows: grid.rows,
      } satisfies UiClientOperation;
      void queueOperation(request).then((result) => {
        if (!active || currentRevisionRef.current !== revision) return;
        if (isStaleOperation(result)) return;
        if (isWorkerFault(result)) {
          if (result.renderRevision === revision
            && (result.runtimeId === undefined || result.runtimeId === activeRuntimeId)) {
            showWorkerFault(result.fault.phase, result.fault.reason, revision, result.runtimeId ?? activeRuntimeId);
          }
          return;
        }
        if (!isFrame(result) || result.renderRevision !== revision) {
          reportCurrentFault('render', 'Client module host returned an invalid resize frame', revision);
          return;
        }
        acceptFrame(result, revision, activeRuntimeId);
      }).catch((error: unknown) => {
        if (active && currentRevisionRef.current === revision) reportCurrentFault('render', errorMessage(error), revision);
      });
    });
    observer.observe(element);
    return () => { active = false; observer.disconnect(); };
  }, [acceptFrame, context, queueOperation, reportCurrentFault, runtimeEpochAvailable, runtimeId, showWorkerFault]);

  const propsSignature = useMemo(() => JSON.stringify(descriptor.props), [descriptor.props]);
  const previousPropsSignature = useRef<string | null>(null);
  const controlAddressable = isClientControlAddressable(descriptor);
  useEffect(() => {
    if (!context || !site || site.committedRevision !== site.renderRevision) return;
    if (!runtimeEpochAvailable || failedRuntimeEpochRef.current === runtimeEpochRef.current) return;
    if (mountPending.current) {
      refreshAfterMount.current = true;
      return;
    }
    const revision = site.renderRevision;
    const currentRuntimeId = runtimeIdRef.current;
    const generation = ++operationGeneration.current;
    let active = true;
    const element = elementRef.current;
    if (!element) return;

    const apply = async () => {
      try {
        if (!currentRuntimeId) {
          const grid = measureGrid(element, false);
          if (!grid) return;
          mountPending.current = true;
          mountOwnerGeneration.current = generation;
          const request = {
            type: 'mount',
            surface: 'desktop',
            component: site.component,
            instance_id: site.instanceId,
            plugin: descriptor.plugin,
            client: descriptor.key,
            module: descriptor.module,
            render_revision: revision,
            columns: grid.columns,
            rows: grid.rows,
          } satisfies UiClientOperation;
          const response = await queueOperation(request);
          if (isStaleOperation(response)) return;
          if (isWorkerFault(response)) {
            if (active && generation === operationGeneration.current && currentRevisionRef.current === revision
              && response.renderRevision === revision) {
              showWorkerFault(response.fault.phase, response.fault.reason, revision, response.runtimeId);
            }
            return;
          }
          if (!isFrame(response)) throw new Error('Client module host returned an invalid frame');
          if (!active || generation !== operationGeneration.current || currentRevisionRef.current !== revision
            || response.renderRevision !== revision) {
            const cleanup = { type: 'unmount', runtimeId: response.runtimeId, render_revision: response.renderRevision } satisfies UiClientOperation;
            void queueOperation(cleanup).catch(() => undefined);
            return;
          }
          if (runtimeIdRef.current !== response.runtimeId) latestFrameSequenceRef.current = 0;
          runtimeIdRef.current = response.runtimeId;
          mountedRevisionRef.current = revision;
          previousPropsSignature.current = propsSignature;
          setRuntimeId(response.runtimeId);
          acceptFrame(response, revision, response.runtimeId);
          return;
        }

        const changedProps = previousPropsSignature.current !== propsSignature;
        const request = changedProps
          ? { type: 'setProps', runtimeId: currentRuntimeId, render_revision: revision, props: descriptor.props } satisfies UiClientOperation
          : { type: 'render', runtimeId: currentRuntimeId, render_revision: revision } satisfies UiClientOperation;
        const response = await queueOperation(request);
        if (!active || generation !== operationGeneration.current || currentRevisionRef.current !== revision) return;
        if (isStaleOperation(response)) return;
        if (isWorkerFault(response)) {
          if (response.renderRevision === revision
            && (response.runtimeId === undefined || response.runtimeId === currentRuntimeId)) {
            showWorkerFault(response.fault.phase, response.fault.reason, revision, response.runtimeId ?? currentRuntimeId);
          }
          return;
        }
        if (!isFrame(response) || response.renderRevision !== revision) throw new Error('Client module host returned a stale frame');
        previousPropsSignature.current = propsSignature;
        mountedRevisionRef.current = revision;
        acceptFrame(response, revision, currentRuntimeId);
      } catch (error) {
        if (!active || generation !== operationGeneration.current || currentRevisionRef.current !== revision) return;
        reportCurrentFault(currentRuntimeId ? 'render' : 'load', errorMessage(error), revision);
      } finally {
        if (!currentRuntimeId && mountOwnerGeneration.current === generation) {
          mountPending.current = false;
          mountOwnerGeneration.current = null;
          if (refreshAfterMount.current) {
            refreshAfterMount.current = false;
            setMeasurementRevision((value) => value + 1);
          }
        }
      }
    };
    void apply();
    return () => {
      active = false;
      if (generation === operationGeneration.current) operationGeneration.current += 1;
    };
  }, [acceptFrame, context, controlAddressable, descriptor, measurementRevision, propsSignature, queueOperation, reportCurrentFault, runtimeEpochAvailable, showWorkerFault, site]);

  const unmountOperationRef = useRef<(runtimeId: string, revision: number) => void>(() => undefined);
  unmountOperationRef.current = (id, revision) => {
    const currentHost = lastHostRef.current;
    const currentSession = lastSessionIdRef.current;
    if (!currentHost || !currentSession) return;
    const request = { type: 'unmount', runtimeId: id, render_revision: revision } satisfies UiClientOperation;
    void currentHost.operation(currentSession, request).catch(() => undefined);
  };
  useEffect(() => () => {
    const currentRuntimeId = runtimeIdRef.current;
    const revision = mountedRevisionRef.current;
    if (!currentRuntimeId || revision === null) return;
    runtimeIdRef.current = null;
    unmountOperationRef.current(currentRuntimeId, revision);
  }, [descriptor.key, descriptor.module, descriptor.plugin, site?.instanceId, site?.sessionId]);

  const onPress = useCallback(async (request: Extract<NativeUiControlRequest, { subtype: 'ui_client_press' }>, _handle: number) => {
    if (!controlAddressable || !context || !runtimeEpochAvailable
      || failedRuntimeEpochRef.current === runtimeEpochRef.current
      || (fault?.source === 'adapter' && fault.phase === 'run')
      || !runtimeIdRef.current || !site || site.committedRevision !== site.renderRevision) return;
    const runtimeAtStart = runtimeIdRef.current;
    const revision = site.renderRevision;
    const generation = operationGeneration.current;
    const result = await context.host.control(site.sessionId, request).catch(() => null);
    // The Native hook/policy dispatch happens before the local VM callback. A
    // failed host dispatch is not evidence that the Client callback failed.
    if (!result) return;
    if (!result.response.reached || runtimeIdRef.current !== runtimeAtStart
      || operationGeneration.current !== generation || currentRevisionRef.current !== revision) return;
    const currentFrame = frameRef.current;
    if (!currentFrame || currentFrame.renderRevision !== revision) return;
    const reachedElement = result.response.reached.element;
    const reachedHandle = heldHandleForElement(currentFrame.tree, reachedElement, descriptor.plugin);
    if (reachedHandle === null) return;
    const event = result.response.reached.value;
    if (request.event.type !== 'press' && typeof event !== 'string') return;
    const operation = {
      type: 'runHeld',
      runtimeId: runtimeAtStart,
      render_revision: revision,
      ...(event === undefined ? {} : { event }),
      handle: reachedHandle,
    } satisfies UiClientOperation;
    const frameResult = await queueOperation(operation);
    if (runtimeIdRef.current !== runtimeAtStart || currentRevisionRef.current !== revision) return;
    if (isStaleOperation(frameResult)) return;
    if (isWorkerFault(frameResult)) {
      if (frameResult.renderRevision === revision
        && (frameResult.runtimeId === undefined || frameResult.runtimeId === runtimeAtStart)) {
        showWorkerFault(frameResult.fault.phase, frameResult.fault.reason, revision, frameResult.runtimeId ?? runtimeAtStart);
      }
      return;
    }
    if (isFrame(frameResult) && frameResult.renderRevision === revision) {
      acceptFrame(frameResult, revision, runtimeAtStart);
    } else {
      throw new Error('Client module host returned an invalid run frame');
    }
  }, [acceptFrame, context, controlAddressable, fault, queueOperation, runtimeEpochAvailable, showWorkerFault, site]);

  const visibleFault = fault && (fault.source === 'worker'
    ? fault.runtimeEpoch !== null && fault.runtimeEpoch === runtimeEpoch
      && (fault.clientStateToken === null || clientStateToken === null || fault.clientStateToken === clientStateToken)
    : (fault.phase === 'run' || site?.renderRevision === fault.revision)
      && (fault.clientStateToken === null || clientStateToken === null || fault.clientStateToken === clientStateToken)) ? fault : null;
  const missingRuntimeEpoch = site !== null && !runtimeEpochAvailable;
  const style = clientElementStyle(descriptor);
  if (visibleFault?.surfaceHeight !== null && visibleFault?.surfaceHeight !== undefined) {
    style.height = `${visibleFault.surfaceHeight}px`;
  }
  return <div ref={elementRef} className="mod-ui-client-instance" data-client={descriptor.key} data-plugin={descriptor.plugin} data-module={descriptor.module} style={style}>
    {visibleFault || missingRuntimeEpoch ? <div className="mod-ui-client-fallback" role="status">This Mod surface is unavailable.</div>
      : frame && frame.renderRevision === site?.renderRevision ? <ModUiClientTree
        tree={frame.tree}
        site={{ component: site?.component ?? 'AbovePrompt', instanceId: site?.instanceId ?? '' }}
        client={descriptor}
        onPress={onPress}
        interactionsEnabled={controlAddressable}
        onRunFault={(request, reason) => {
          const revision = site?.instanceId === request.instance_id ? site.renderRevision : -1;
          reportCurrentFault('run', reason, revision);
        }}
        onRenderFault={(reason) => reportCurrentFault('render', reason, site?.renderRevision ?? -1)}
        renderRevision={frame.renderRevision}
      />
        : <div className="mod-ui-client-loading" role="status" aria-label="Loading Mod surface" />}
  </div>;
}
