import { createElement, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';

import type { ModUiClientFrameSnapshot, ModUiControlRequest, UseBridge } from '../bridge/bridgeTypes.js';
import { useT } from '../theme/ThemeContext.js';
import { MarkdownContent } from './MarkdownContent.js';

const ABOVE_PROMPT_INSTANCE_ID = 'electron-above-prompt';
const MAX_UI_DEPTH = 32;
const MAX_UI_NODES = 20_000;
const MAX_UI_TEXT_UNITS = 10_000;
const MAX_CONTROL_VALUE_UNITS = 16_384;
const MAX_SELECT_OPTIONS = 64;

type JsonObject = Record<string, unknown>;
export interface ModUiViewport { readonly columns: number; readonly rows: number }

interface RenderedTree {
  readonly tree: unknown;
  readonly sequence: number;
  readonly renderRevision: number;
}

interface ClientFrameScope {
  readonly runtimeId: string;
  readonly renderRevision: number;
  readonly plugin: string;
  readonly key: string;
  readonly module: string;
  readonly hasPointerListener: boolean;
  readonly hasKeyListener: boolean;
  readonly parent: { readonly surface: 'desktop'; readonly component: 'AbovePrompt'; readonly instance_id: string };
}

interface ClientFrameView {
  readonly snapshot: ModUiClientFrameSnapshot;
  readonly frame: JsonObject;
  readonly scope: ClientFrameScope;
}

interface PendingClientPointer {
  scope: ClientFrameScope;
  event: JsonObject | null;
  frame: number | null;
  busy: boolean;
}

function isRecord(value: unknown): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function safeInteger(value: unknown, minimum = 0, maximum = 10_000): value is number {
  return Number.isSafeInteger(value) && (value as number) >= minimum && (value as number) <= maximum;
}

function isPressIdentity(value: unknown): value is JsonObject & { plugin: string; handle: number } {
  return isRecord(value) && typeof value['plugin'] === 'string' && value['plugin'].length > 0
    && value['plugin'].length <= 256 && safeInteger(value['handle'], 1, Number.MAX_SAFE_INTEGER);
}

export function isValidModUiTree(tree: unknown): boolean {
  let count = 0;
  const visit = (node: unknown, depth: number): boolean => {
    if (typeof node === 'string') return node.length <= MAX_UI_TEXT_UNITS;
    if (!isRecord(node) || depth > MAX_UI_DEPTH || ++count > MAX_UI_NODES) return false;
    const type = node['type'];
    if (typeof type !== 'string' || ![
      'engine', 'Box', 'Text', 'div', 'span', 'b', 'Button', 'Input', 'Select',
      'Link', 'Code', 'Markdown', 'Svg', 'Client',
    ].includes(type)) return false;
    const props = node['props'];
    if (props !== undefined && !isRecord(props)) return false;
    if (type === 'engine') return safeInteger(node['ref'], 1, Number.MAX_SAFE_INTEGER);
    const children = node['children'];
    if (children !== undefined && (!Array.isArray(children) || !children.every((child) => visit(child, depth + 1)))) {
      return false;
    }
    if (type === 'Button') {
      const press = node['press'];
      const held = node['held'];
      return isRecord(props) && typeof props['key'] === 'string' && props['key'].length > 0
        && props['key'].length <= 256 && typeof props['label'] === 'string'
        && props['label'].length <= MAX_UI_TEXT_UNITS
        && (isPressIdentity(press) || safeInteger(held, 1, Number.MAX_SAFE_INTEGER));
    }
    if (type === 'Input') {
      const press = node['press'];
      const held = node['held'];
      return isRecord(props) && typeof props['key'] === 'string' && props['key'].length > 0
        && props['key'].length <= 256
        && (isPressIdentity(press) || safeInteger(held, 1, Number.MAX_SAFE_INTEGER))
        && ['label', 'placeholder', 'value', 'submitLabel'].every((key) => props[key] === undefined
          || typeof props[key] === 'string' && props[key].length <= MAX_CONTROL_VALUE_UNITS);
    }
    if (type === 'Select') {
      const press = node['press'];
      const held = node['held'];
      const options = props?.['options'];
      return isRecord(props) && typeof props['key'] === 'string' && props['key'].length > 0
        && props['key'].length <= 256 && Array.isArray(options) && options.length > 0
        && options.length <= MAX_SELECT_OPTIONS && options.every((option) => isRecord(option)
          && typeof option['value'] === 'string' && option['value'].length <= MAX_CONTROL_VALUE_UNITS
          && (option['label'] === undefined || typeof option['label'] === 'string'
            && option['label'].length <= MAX_UI_TEXT_UNITS))
        && (isPressIdentity(press) || safeInteger(held, 1, Number.MAX_SAFE_INTEGER));
    }
    if (type === 'Link') {
      return isRecord(props) && typeof props['href'] === 'string' && props['href'].length <= 2048
        && (props['label'] === undefined || typeof props['label'] === 'string'
          && props['label'].length <= MAX_UI_TEXT_UNITS);
    }
    if (type === 'Markdown') {
      return isRecord(props) && typeof props['text'] === 'string'
        && props['text'].length <= MAX_UI_TEXT_UNITS
        && (props['pressableLinks'] === undefined || Array.isArray(props['pressableLinks'])
          && props['pressableLinks'].length <= 256
          && props['pressableLinks'].every((href) => typeof href === 'string' && href.length <= 2_048))
        && (node['press'] === undefined || isPressIdentity(node['press']));
    }
    if (type === 'Code') {
      return isRecord(props) && typeof props['source'] === 'string'
        && props['source'].length <= MAX_UI_TEXT_UNITS;
    }
    if (type === 'Svg') {
      return isRecord(props) && typeof props['source'] === 'string'
        && props['source'].length <= 100_000
        && typeof props['alt'] === 'string' && props['alt'].length <= MAX_UI_TEXT_UNITS
        && ['width', 'height'].every((name) => props[name] === undefined
          || typeof props[name] === 'number' && Number.isFinite(props[name]) && props[name] >= 0 && props[name] <= 10_000
          || typeof props[name] === 'string' && /^\d{1,3}%$/.test(props[name]))
        && (props['isInteractive'] === undefined || typeof props['isInteractive'] === 'boolean');
    }
    if (type === 'Client') {
      const client = node['client'];
      return isRecord(props) && isRecord(client) && typeof client['plugin'] === 'string'
        && client['plugin'].length > 0 && client['plugin'].length <= 256
        && typeof props['key'] === 'string' && props['key'].length > 0 && props['key'].length <= 10_000
        && typeof props['module'] === 'string' && props['module'].length > 0
        && props['module'].length <= 10_000;
    }
    return true;
  };

  return tree === null || (Array.isArray(tree)
    ? tree.every((node) => visit(node, 0))
    : visit(tree, 0));
}

export function abovePromptRequest(clientId: string, isWorking: boolean, viewport: ModUiViewport): JsonObject {
  const columns = Math.max(1, viewport.columns);
  const rows = Math.max(1, viewport.rows);
  const maxRows = Math.min(rows, 12);
  return {
    subtype: 'ui_render',
    surface: 'desktop',
    component: 'AbovePrompt',
    instance_id: ABOVE_PROMPT_INSTANCE_ID,
    client_id: clientId,
    props: {
      hasSurvey: false,
      isWorking,
      maxRows,
      bodyColumns: columns,
      scroll: { offset: 0, bodyRows: maxRows },
      view: {},
    },
    viewport: { columns, rows, isFullscreen: false },
  };
}

function requestJsonCommand(type: 'ui_render' | 'ui_press' | 'ui_input' | 'ui_select', body: JsonObject): ModUiControlRequest {
  return { type, request_json: JSON.stringify(body) } as ModUiControlRequest;
}

function uiStyle(type: string, props: JsonObject | undefined): CSSProperties {
  if (!props) return {};
  const style: CSSProperties = {};
  if (type === 'Box') {
    if (typeof props['flexDirection'] === 'string') style.flexDirection = props['flexDirection'] as CSSProperties['flexDirection'];
    if (typeof props['flexGrow'] === 'number') style.flexGrow = props['flexGrow'];
    if (typeof props['flexShrink'] === 'number') style.flexShrink = props['flexShrink'];
    if (typeof props['flexWrap'] === 'string') style.flexWrap = props['flexWrap'] as CSSProperties['flexWrap'];
    if (typeof props['alignItems'] === 'string') style.alignItems = props['alignItems'] as CSSProperties['alignItems'];
    if (typeof props['alignSelf'] === 'string') style.alignSelf = props['alignSelf'] as CSSProperties['alignSelf'];
    if (typeof props['justifyContent'] === 'string') style.justifyContent = props['justifyContent'] as CSSProperties['justifyContent'];
    for (const key of ['gap', 'columnGap', 'rowGap', 'margin', 'marginTop', 'marginBottom', 'padding', 'paddingTop', 'paddingBottom'] as const) {
      if (typeof props[key] === 'number') style[key] = props[key];
    }
    for (const key of ['width', 'height', 'minWidth', 'minHeight'] as const) {
      const value = props[key];
      if (typeof value === 'number' || typeof value === 'string' && /^\d{1,3}%$/.test(value)) style[key] = value;
    }
    if (props['display'] === 'none' || props['display'] === 'flex') style.display = props['display'];
    if (props['position'] === 'relative' || props['position'] === 'absolute') style.position = props['position'];
    if (props['overflow'] === 'hidden' || props['overflow'] === 'visible') style.overflow = props['overflow'];
    if (typeof props['backgroundColor'] === 'string') style.backgroundColor = props['backgroundColor'];
    if (typeof props['borderColor'] === 'string') style.borderColor = props['borderColor'];
    if (typeof props['borderStyle'] === 'string') style.borderStyle = 'solid';
    if (typeof props['borderStyle'] === 'string') style.borderWidth = 1;
    if (typeof props['borderRadius'] === 'number') style.borderRadius = props['borderRadius'];
    return style;
  }
  if (type === 'Text') {
    if (typeof props['color'] === 'string') style.color = props['color'];
    if (typeof props['backgroundColor'] === 'string') style.backgroundColor = props['backgroundColor'];
    if (props['bold'] === true) style.fontWeight = 650;
    if (props['italic'] === true) style.fontStyle = 'italic';
    if (props['underline'] === true || props['strikethrough'] === true) {
      style.textDecoration = [props['underline'] === true ? 'underline' : '', props['strikethrough'] === true ? 'line-through' : '']
        .filter(Boolean).join(' ');
    }
    if (props['dimColor'] === true) style.opacity = .7;
    if (props['wrap'] === 'truncate' || props['wrap'] === 'truncate-end' || props['wrap'] === 'truncate-start') {
      style.overflow = 'hidden';
      style.textOverflow = 'ellipsis';
      style.whiteSpace = 'nowrap';
    } else if (props['wrap'] === 'wrap') {
      style.whiteSpace = 'pre-wrap';
    }
  }
  return style;
}

function controlIdentity(node: JsonObject, clientId: string): JsonObject | null {
  const press = node['press'];
  const props = node['props'];
  if (!isRecord(press) || !isRecord(props) || typeof props['key'] !== 'string'
    || typeof press['plugin'] !== 'string' || typeof press['handle'] !== 'number') return null;
  return {
    plugin: press['plugin'],
    handle: press['handle'],
    key: props['key'],
    component: 'AbovePrompt',
    instance_id: ABOVE_PROMPT_INSTANCE_ID,
    surface: 'desktop',
    client_id: clientId,
  };
}

function safeExternalHref(value: unknown): string | null {
  if (typeof value !== 'string' || value.length > 2048) return null;
  try {
    const url = new URL(value);
    return url.protocol === 'https:' || url.protocol === 'http:' || url.protocol === 'mailto:'
      ? value : null;
  } catch {
    return null;
  }
}

const SAFE_SVG_TAGS = new Set([
  'svg', 'g', 'path', 'rect', 'circle', 'ellipse', 'line', 'polyline', 'polygon',
  'defs', 'linearGradient', 'radialGradient', 'stop', 'clipPath', 'mask', 'text', 'tspan', 'title', 'desc', 'use',
]);
const SAFE_SVG_ATTRIBUTES: Readonly<Record<string, string>> = {
  id: 'id', viewBox: 'viewBox', preserveAspectRatio: 'preserveAspectRatio',
  x: 'x', y: 'y', x1: 'x1', y1: 'y1', x2: 'x2', y2: 'y2', cx: 'cx', cy: 'cy',
  r: 'r', rx: 'rx', ry: 'ry', width: 'width', height: 'height', d: 'd', points: 'points',
  fill: 'fill', fillRule: 'fillRule', stroke: 'stroke', strokeWidth: 'strokeWidth',
  strokeLinecap: 'strokeLinecap', strokeLinejoin: 'strokeLinejoin', strokeDasharray: 'strokeDasharray',
  strokeDashoffset: 'strokeDashoffset', opacity: 'opacity', fillOpacity: 'fillOpacity',
  strokeOpacity: 'strokeOpacity', transform: 'transform', offset: 'offset', stopColor: 'stopColor',
  stopOpacity: 'stopOpacity', gradientUnits: 'gradientUnits', gradientTransform: 'gradientTransform',
  clipPath: 'clipPath', mask: 'mask', href: 'href',
  'fill-rule': 'fillRule', 'stroke-width': 'strokeWidth', 'stroke-linecap': 'strokeLinecap',
  'stroke-linejoin': 'strokeLinejoin', 'stroke-dasharray': 'strokeDasharray',
  'stroke-dashoffset': 'strokeDashoffset', 'fill-opacity': 'fillOpacity',
  'stroke-opacity': 'strokeOpacity', 'stop-color': 'stopColor', 'stop-opacity': 'stopOpacity',
  'gradient-units': 'gradientUnits', 'gradient-transform': 'gradientTransform',
  'clip-path': 'clipPath',
};

function safeSvgAttribute(name: string, value: string): string | null {
  if (value.length > 10_000 || /[<>\u0000-\u001f]/.test(value)) return null;
  if (name === 'id') return /^[A-Za-z_][A-Za-z0-9_.-]{0,127}$/.test(value) ? value : null;
  if (name === 'href') return /^#[A-Za-z_][A-Za-z0-9_.-]{0,127}$/.test(value) ? value : null;
  if (name === 'clip-path' || name === 'mask') {
    return /^url\(#[A-Za-z_][A-Za-z0-9_.-]{0,127}\)$/.test(value) ? value : null;
  }
  if (['fill', 'stroke', 'stop-color'].includes(name)) {
    return /^(?:none|currentColor|transparent|#[0-9a-f]{3,8}|[a-z]{1,24}|(?:rgb|rgba|hsl|hsla)\([0-9.,%\s]+\))$/i.test(value)
      ? value : null;
  }
  if (name === 'transform' || name === 'gradientTransform') {
    return /^(?:(?:matrix|translate|scale|rotate|skewX|skewY)\([-+0-9.eE,\s]+\)\s*)+$/.test(value)
      ? value : null;
  }
  if (name === 'viewBox') return /^[-+0-9.eE,\s]+$/.test(value) ? value : null;
  return value;
}

function safeSvgTree(source: string, alt: string, width: unknown, height: unknown): ReactNode {
  if (source.length > 100_000 || typeof DOMParser === 'undefined') return null;
  try {
    const document = new DOMParser().parseFromString(source, 'image/svg+xml');
    const root = document.documentElement;
    if (root.localName !== 'svg' || document.querySelector('parsererror')) return null;
    let nodeCount = 0;
    const visit = (node: Element, depth: number, key: string): ReactNode => {
      if (depth > 32 || ++nodeCount > 2_000 || !SAFE_SVG_TAGS.has(node.localName)) return null;
      const props: Record<string, string | number> = {};
      for (const attribute of Array.from(node.attributes)) {
        const reactName = SAFE_SVG_ATTRIBUTES[attribute.name];
        if (!reactName) continue;
        const value = safeSvgAttribute(attribute.name, attribute.value);
        if (value !== null) props[reactName] = value;
      }
      if (node === root) {
        props['role'] = 'img';
        props['aria-label'] = alt;
        if (typeof width === 'number' || typeof width === 'string' && /^\d{1,3}%$/.test(width)) props['width'] = width;
        if (typeof height === 'number' || typeof height === 'string' && /^\d{1,3}%$/.test(height)) props['height'] = height;
      }
      const children: ReactNode[] = [];
      for (const [index, child] of Array.from(node.childNodes).entries()) {
        if (child.nodeType === Node.TEXT_NODE) {
          if ((child.textContent?.length ?? 0) <= MAX_UI_TEXT_UNITS) children.push(child.textContent ?? '');
        } else if (child.nodeType === Node.ELEMENT_NODE) {
          const rendered = visit(child as Element, depth + 1, `${key}.${index}`);
          if (rendered !== null) children.push(rendered);
        }
      }
      return createElement(node.localName, { ...props, key }, ...children);
    };
    return visit(root, 0, 'svg');
  } catch {
    return null;
  }
}

export function clientFrameForNode(
  snapshots: readonly ModUiClientFrameSnapshot[],
  sessionId: string,
  renderRevision: number,
  node: JsonObject,
): ClientFrameView | null {
  const props = node['props'];
  const client = node['client'];
  if (!isRecord(props) || !isRecord(client)
    || typeof props['key'] !== 'string' || typeof props['module'] !== 'string'
    || typeof client['plugin'] !== 'string') return null;

  for (let index = snapshots.length - 1; index >= 0; index -= 1) {
    const snapshot = snapshots[index];
    if (!snapshot || snapshot.sessionId !== sessionId) continue;
    const frame = snapshot.frame;
    if (!isRecord(frame) || frame['renderRevision'] !== renderRevision
      || !isRecord(frame['parent']) || !isRecord(frame['client'])
      || frame['parent']['surface'] !== 'desktop'
      || frame['parent']['component'] !== 'AbovePrompt'
      || frame['parent']['instance_id'] !== ABOVE_PROMPT_INSTANCE_ID
      || frame['client']['plugin'] !== client['plugin']
      || frame['client']['key'] !== props['key']
      || frame['client']['module'] !== props['module']
      || typeof frame['hasPointerListener'] !== 'boolean'
      || typeof frame['hasKeyListener'] !== 'boolean'
      || !isValidModUiTree(frame['tree'])) continue;
    return {
      snapshot,
      frame,
      scope: {
        runtimeId: snapshot.runtimeId,
        renderRevision,
        plugin: client['plugin'],
        key: props['key'],
        module: props['module'],
        hasPointerListener: frame['hasPointerListener'],
        hasKeyListener: frame['hasKeyListener'],
        parent: { surface: 'desktop', component: 'AbovePrompt', instance_id: ABOVE_PROMPT_INSTANCE_ID },
      },
    };
  }
  return null;
}

export function clientOperationRequest(operation: JsonObject): ModUiControlRequest {
  return {
    type: 'ui_client_operation',
    operation_json: JSON.stringify(operation),
  } as ModUiControlRequest;
}

function pointerPayload(event: ReactPointerEvent<HTMLDivElement>): JsonObject {
  const output: JsonObject = { type: event.type, pointerId: event.pointerId, pointerType: event.pointerType };
  for (const key of ['button', 'buttons', 'clientX', 'clientY', 'pressure'] as const) {
    const value = event[key];
    if (typeof value === 'number' && Number.isFinite(value)) output[key] = value;
  }
  for (const key of ['shiftKey', 'ctrlKey', 'altKey', 'metaKey'] as const) output[key] = event[key];
  return output;
}

function keyPayload(event: ReactKeyboardEvent<HTMLDivElement>): JsonObject {
  const output: JsonObject = { type: event.type };
  if (event.key.length <= 128) output['key'] = event.key;
  if (event.code.length <= 128) output['code'] = event.code;
  output['repeat'] = event.repeat;
  output['shiftKey'] = event.shiftKey;
  output['ctrlKey'] = event.ctrlKey;
  output['altKey'] = event.altKey;
  output['metaKey'] = event.metaKey;
  return output;
}

export function ModUiSurface({ bridge, isWorking }: { readonly bridge: UseBridge; readonly isWorking: boolean }) {
  const t = useT();
  const host = useRef<HTMLDivElement>(null);
  const renderSequence = useRef(0);
  const mounted = useRef(false);
  const [viewport, setViewport] = useState<ModUiViewport>({ columns: 80, rows: 12 });
  const [rendered, setRendered] = useState<RenderedTree | null>(null);
  const [renderError, setRenderError] = useState<string | null>(null);
  const [controlError, setControlError] = useState<string | null>(null);
  const [pendingControls, setPendingControls] = useState<ReadonlySet<string>>(() => new Set());
  const [draftValues, setDraftValues] = useState<Readonly<Record<string, string>>>({});
  const clientPointerQueues = useRef(new Map<string, PendingClientPointer>());
  const sessionId = bridge.activeSession?.sessionId ?? null;

  useLayoutEffect(() => {
    const element = host.current;
    if (!element) return;
    const update = () => {
      const bounds = element.getBoundingClientRect();
      const next = {
        columns: Math.max(1, Math.floor(bounds.width)),
        rows: Math.max(1, Math.floor(bounds.height)),
      };
      setViewport((previous) => previous.columns === next.columns && previous.rows === next.rows ? previous : next);
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      for (const queue of clientPointerQueues.current.values()) {
        if (queue.frame !== null) cancelAnimationFrame(queue.frame);
      }
      clientPointerQueues.current.clear();
    };
  }, [sessionId]);

  useEffect(() => {
    if (!sessionId || !bridge.connected || !bridge.modUiSurfaceReady) {
      setRendered(null);
      return;
    }
    let current = true;
    const sequence = ++renderSequence.current;
    setRenderError(null);
    const request = abovePromptRequest(bridge.modUiClientId, isWorking, viewport);
    void bridge.requestModUiControl(requestJsonCommand('ui_render', request)).then(({ response }) => {
      if (!current || sequence !== renderSequence.current) return;
      if (!isRecord(response)) {
        setRendered(null);
        setRenderError('The Mod UI renderer returned no tree.');
        return;
      }
      const metadata = response.metadata;
      const renderRevision = isRecord(metadata) && safeInteger(metadata['renderRevision'], 1, Number.MAX_SAFE_INTEGER)
        ? metadata['renderRevision'] : null;
      if (renderRevision === null) {
        setRendered(null);
        setRenderError('The Mod UI renderer returned no revision.');
        return;
      }
      const tree = response['tree'];
      if (!isValidModUiTree(tree)) {
        setRendered(null);
        setRenderError('The Mod UI tree exceeded the desktop renderer limits.');
        return;
      }
      setDraftValues({});
      setControlError(null);
      setRendered({ tree, sequence, renderRevision });
    }).catch((cause: unknown) => {
      if (!current || sequence !== renderSequence.current) return;
      setRendered(null);
      setRenderError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => {
      current = false;
      renderSequence.current += 1;
    };
  }, [bridge.connected, bridge.modUiClientId, bridge.modUiInvalidationRevision,
    bridge.modUiSurfaceReady, bridge.requestModUiControl, isWorking, sessionId, viewport]);

  if (!sessionId || !bridge.connected || !bridge.modUiSurfaceReady) return null;
  const tree = rendered?.tree;
  if (isRecord(tree) && tree['type'] === 'engine') return null;
  if ((tree === null || tree === undefined) && !renderError && !controlError) return null;

  const issueControl = async (key: string, request: ModUiControlRequest): Promise<void> => {
    if (!mounted.current) return;
    setControlError(null);
    setPendingControls((previous) => new Set(previous).add(key));
    try {
      await bridge.requestModUiControl(request);
    } catch (cause) {
      if (mounted.current) setControlError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (mounted.current) {
        setPendingControls((previous) => {
          const next = new Set(previous);
          next.delete(key);
          return next;
        });
      }
    }
  };

  const sendClientOperation = async (
    scope: ClientFrameScope,
    operation: 'runHeld' | 'pointer' | 'key',
    fields: JsonObject,
    pendingKey?: string,
  ): Promise<void> => {
    if (!sessionId || !mounted.current) return;
    if (pendingKey) setPendingControls((previous) => new Set(previous).add(pendingKey));
    try {
      const { response } = await bridge.requestModUiControl(clientOperationRequest({
        type: operation,
        runtimeId: scope.runtimeId,
        render_revision: scope.renderRevision,
        ...fields,
      }));
      if (!isRecord(response) || response['runtimeId'] !== scope.runtimeId
        || response['renderRevision'] !== scope.renderRevision || !isValidModUiTree(response['tree'])) {
        bridge.forgetModUiClientFrame(sessionId, scope.runtimeId, scope.renderRevision);
        return;
      }
      if (!mounted.current) return;
      bridge.rememberModUiClientFrame({
        sessionId,
        runtimeId: scope.runtimeId,
        frame: {
          ...response,
          parent: scope.parent,
          client: { plugin: scope.plugin, key: scope.key, module: scope.module },
        },
      });
    } catch (cause) {
      bridge.forgetModUiClientFrame(sessionId, scope.runtimeId, scope.renderRevision);
      if (mounted.current) setControlError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (pendingKey && mounted.current) {
        setPendingControls((previous) => {
          const next = new Set(previous);
          next.delete(pendingKey);
          return next;
        });
      }
    }
  };

  const sendClientPointer = (scope: ClientFrameScope, event: JsonObject): void => {
    const queue = clientPointerQueues.current.get(scope.runtimeId) ?? {
      scope, event: null, frame: null, busy: false,
    };
    queue.scope = scope;
    queue.event = event;
    clientPointerQueues.current.set(scope.runtimeId, queue);
    const schedule = () => {
      if (queue.busy || queue.frame !== null || queue.event === null) return;
      queue.frame = requestAnimationFrame(() => {
        queue.frame = null;
        const nextEvent = queue.event;
        queue.event = null;
        if (!nextEvent) return;
        queue.busy = true;
        void sendClientOperation(queue.scope, 'pointer', { event: nextEvent }).finally(() => {
          queue.busy = false;
          schedule();
        });
      });
    };
    schedule();
  };

  const renderNode = (node: unknown, key: string, depth = 0, clientScope?: ClientFrameScope): ReactNode => {
    if (typeof node === 'string') return node;
    if (!isRecord(node) || depth > MAX_UI_DEPTH) return null;
    const type = node['type'];
    const props = isRecord(node['props']) ? node['props'] : undefined;
    const children = Array.isArray(node['children'])
      ? node['children'].map((child, index) => renderNode(child, `${key}.${index}`, depth + 1, clientScope)) : [];
    if (type === 'engine') return null;
    if (type === 'Box') {
      return <div key={key} style={{ display: 'flex', flexDirection: 'column', ...uiStyle('Box', props) }}>{children}</div>;
    }
    if (type === 'Text') return <span key={key} style={uiStyle('Text', props)}>{children}</span>;
    if (type === 'div' || type === 'span' || type === 'b') {
      if (type === 'div') return <div key={key}>{children}</div>;
      if (type === 'span') return <span key={key}>{children}</span>;
      return <strong key={key}>{children}</strong>;
    }
    if (type === 'Button' && props) {
      const identity = controlIdentity(node, bridge.modUiClientId);
      const held = safeInteger(node['held'], 1, Number.MAX_SAFE_INTEGER) ? node['held'] : null;
      const label = typeof props['label'] === 'string' ? props['label'] : '';
      const busy = pendingControls.has(key);
      return <button
        key={key}
        type="button"
        aria-label={label || undefined}
        disabled={clientScope ? held === null || busy : !identity || busy}
        onClick={() => {
          if (clientScope && held !== null) {
            void sendClientOperation(clientScope, 'runHeld', { handle: held, event: { type: 'press' } }, key);
          } else if (identity) {
            void issueControl(key, requestJsonCommand('ui_press', { subtype: 'ui_press', ...identity }));
          }
        }}
        style={{
          border: `1px solid ${t.border}`,
          borderRadius: 8,
          padding: '6px 11px',
          background: props['variant'] === 'primary' ? t.accent : t.surface,
          color: props['variant'] === 'primary' ? '#fff' : t.text,
          opacity: props['dimColor'] === true ? .7 : 1,
          cursor: (clientScope ? held !== null : identity !== null) && !busy ? 'pointer' : 'wait',
          ...uiStyle('Text', props),
        }}
      >{Array.isArray(node['children']) && node['children'].length > 0 ? children : label}</button>;
    }
    if (type === 'Input' && props) {
      const identity = controlIdentity(node, bridge.modUiClientId);
      const held = safeInteger(node['held'], 1, Number.MAX_SAFE_INTEGER) ? node['held'] : null;
      const keyName = typeof props['key'] === 'string' ? props['key'] : key;
      const draftKey = `${rendered?.sequence ?? 0}:${keyName}`;
      const value = draftValues[draftKey] ?? (typeof props['value'] === 'string' ? props['value'] : '');
      const label = typeof props['label'] === 'string' ? props['label'] : undefined;
      const placeholder = typeof props['placeholder'] === 'string' ? props['placeholder'] : undefined;
      return <label key={key} style={{ display: 'flex', flexDirection: 'column', gap: 4, color: t.text, fontSize: 12 }}>
        {label}
        <input
          type="text"
          value={value}
          maxLength={MAX_CONTROL_VALUE_UNITS}
          placeholder={placeholder}
          disabled={clientScope ? held === null : !identity}
          onChange={(event) => {
            const next = event.currentTarget.value;
            setDraftValues((previous) => ({ ...previous, [draftKey]: next }));
            if (clientScope && held !== null && next.length <= MAX_CONTROL_VALUE_UNITS) {
              void sendClientOperation(clientScope, 'runHeld', { handle: held, event: { kind: 'change', value: next } });
            } else if (identity && next.length <= MAX_CONTROL_VALUE_UNITS) {
              void issueControl(key, requestJsonCommand('ui_input', {
                subtype: 'ui_input', ...identity, kind: 'change', value: next,
              }));
            }
          }}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && clientScope && held !== null) {
              void sendClientOperation(clientScope, 'runHeld', { handle: held, event: { kind: 'submit', value } }, key);
            } else if (event.key === 'Enter' && identity) {
              void issueControl(key, requestJsonCommand('ui_input', {
                subtype: 'ui_input', ...identity, kind: 'submit', value,
              }));
            }
          }}
          style={{ border: `1px solid ${t.border}`, borderRadius: 7, padding: '7px 9px', color: t.text, background: t.surface }}
        />
      </label>;
    }
    if (type === 'Select' && props) {
      const identity = controlIdentity(node, bridge.modUiClientId);
      const held = safeInteger(node['held'], 1, Number.MAX_SAFE_INTEGER) ? node['held'] : null;
      const options = Array.isArray(props['options']) ? props['options'].filter(isRecord) : [];
      const keyName = typeof props['key'] === 'string' ? props['key'] : key;
      const draftKey = `${rendered?.sequence ?? 0}:${keyName}`;
      const selected = draftValues[draftKey] ?? (typeof props['value'] === 'string' ? props['value'] : '');
      const label = typeof props['label'] === 'string' ? props['label'] : undefined;
      return <label key={key} style={{ display: 'flex', flexDirection: 'column', gap: 4, color: t.text, fontSize: 12 }}>
        {label}
        <select
          value={selected}
          disabled={clientScope ? held === null : !identity}
          onChange={(event) => {
            const value = event.currentTarget.value;
            setDraftValues((previous) => ({ ...previous, [draftKey]: value }));
            if (clientScope && held !== null && value.length <= MAX_CONTROL_VALUE_UNITS) {
              void sendClientOperation(clientScope, 'runHeld', { handle: held, event: { value } }, key);
            } else if (identity && value.length <= MAX_CONTROL_VALUE_UNITS) {
              void issueControl(key, requestJsonCommand('ui_select', { subtype: 'ui_select', ...identity, value }));
            }
          }}
          style={{ border: `1px solid ${t.border}`, borderRadius: 7, padding: '7px 9px', color: t.text, background: t.surface }}
        >
          {options.map((option, index) => typeof option['value'] === 'string'
            ? <option key={`${key}.${index}`} value={option['value']}>
              {typeof option['label'] === 'string' ? option['label'] : option['value']}
            </option> : null)}
        </select>
      </label>;
    }
    if (type === 'Link' && props) {
      const href = safeExternalHref(props['href']);
      const label = typeof props['label'] === 'string' ? props['label'] : href;
      return href ? <a key={key} href={href} target="_blank" rel="noreferrer" style={{ color: t.accent }}>{label}</a>
        : <span key={key}>{label ?? 'Link unavailable'}</span>;
    }
    if (type === 'Markdown' && props && typeof props['text'] === 'string') {
      const identity = !clientScope ? controlIdentity(node, bridge.modUiClientId) : null;
      const onLinkPress = identity ? (href: string) => {
        void issueControl(key, requestJsonCommand('ui_press', {
          subtype: 'ui_press', ...identity, href,
        }));
      } : undefined;
      return <div key={key} style={{ color: t.text, opacity: props['dimColor'] === true ? .7 : 1 }}>
        <MarkdownContent text={props['text']} onLinkPress={onLinkPress} />
      </div>;
    }
    if (type === 'Code' && props && typeof props['source'] === 'string') {
      return <pre key={key} style={{ margin: 0, overflowX: 'auto', padding: 8, borderRadius: 7, background: t.surfaceHover, color: t.text }}>
        <code>{props['source']}</code>
      </pre>;
    }
    if (type === 'Svg' && props && typeof props['alt'] === 'string') {
      if (typeof props['source'] === 'string') {
        const svg = safeSvgTree(props['source'], props['alt'], props['width'], props['height']);
        if (svg !== null) return <span key={key} className="mod-ui-svg">{svg}</span>;
      }
      return <span key={key} role="img" aria-label={props['alt']} />;
    }
    if (type === 'Client' && props && typeof props['module'] === 'string') {
      const view = rendered && clientFrameForNode(
        bridge.modUiClientFrames,
        sessionId ?? '',
        rendered.renderRevision,
        node,
      );
      if (!view) return <span key={key} role="status" style={{ color: t.text3, fontSize: 11 }}>Loading {props['module']}…</span>;
      const childTree = view.frame['tree'];
      const childNodes = Array.isArray(childTree) ? childTree : [childTree];
      const style: CSSProperties = { minWidth: 0, flexGrow: 1 };
      for (const dimension of ['width', 'height'] as const) {
        const value = props[dimension];
        if (typeof value === 'number' || typeof value === 'string' && /^\d{1,3}%$/.test(value)) style[dimension] = value;
      }
      return <div
        key={key}
        className="mod-ui-client-surface"
        data-runtime-id={view.scope.runtimeId}
        style={style}
        tabIndex={view.scope.hasKeyListener ? 0 : undefined}
        onPointerDown={view.scope.hasPointerListener ? (event) => sendClientPointer(view.scope, pointerPayload(event)) : undefined}
        onPointerMove={view.scope.hasPointerListener ? (event) => sendClientPointer(view.scope, pointerPayload(event)) : undefined}
        onPointerUp={view.scope.hasPointerListener ? (event) => sendClientPointer(view.scope, pointerPayload(event)) : undefined}
        onKeyDown={view.scope.hasKeyListener ? (event) => {
          void sendClientOperation(view.scope, 'key', { event: keyPayload(event) });
        } : undefined}
      >{childNodes.map((child, index) => renderNode(child, `${key}.client.${index}`, depth + 1, view.scope))}</div>;
    }
    return null;
  };

  return <div ref={host} className="mod-ui-above-prompt" data-session-id={sessionId}
    style={{ display: 'flex', flexDirection: 'column', gap: 8, width: '100%', maxWidth: 'var(--conversation-width)', margin: '0 auto 8px', color: t.text }}>
    {tree === undefined ? renderError && <div role="status" style={{ color: t.text3, fontSize: 11 }}>{renderError}</div> : renderNode(tree, 'root')}
    {controlError && <div role="status" style={{ color: t.warn, fontSize: 11 }}>{controlError}</div>}
  </div>;
}
