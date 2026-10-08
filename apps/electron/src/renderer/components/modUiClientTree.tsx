import * as React from 'react';
import { Component, memo, useCallback, useEffect, useRef, useState, type ErrorInfo, type ReactNode } from 'react';
import type { CSSProperties, FormEvent, KeyboardEvent, ReactElement, ChangeEvent } from 'react';
import type { NativeUiComponent, NativeUiControlRequest, NativeUiClientPressEventDto, UiJsonValue } from '@lingxi/bridge-client';
import { useT } from '../theme/ThemeContext';
import { CodeBlock } from './CodeBlock';
import { MarkdownContent } from './MarkdownContent';

export type ModUiJsonValue = UiJsonValue;
export type ModUiSurfaceNodeType = 'Box' | 'Text' | 'Button' | 'Input' | 'Select' | 'Link' | 'Code' | 'Markdown';

export interface ModUiRenderSite {
  component: NativeUiComponent;
  /** The parent ui.render instance id, not the derived Client drawing id. */
  instanceId: string;
}

export interface ModUiClientIdentity {
  plugin: string;
  /** The parent Client key, sent as `client` in ui_client_press. */
  key: string;
  /** The plugin-relative module key from the validated parent render tree. */
  module: string;
}

export type ModUiClientPressEvent = NativeUiClientPressEventDto;
/** Exact bridge-client request consumed by Native's `ui_client_press` control. */
export type NativeUiClientPressRequest = Extract<NativeUiControlRequest, { subtype: 'ui_client_press' }>;

export interface ModUiClientPressRef {
  plugin: string;
  handle: number;
}

interface SelectOption {
  value: string;
  label?: string;
}

type Primitive = string | number | boolean;
type PrimitiveRecord = Readonly<Record<string, Primitive>>;
type SurfaceChild = string | ModUiSurfaceNode;

export type ModUiSurfaceNode =
  | { type: 'Box' | 'Text'; props?: PrimitiveRecord; hover?: ModUiJsonValue; children?: readonly SurfaceChild[] }
  | {
      type: 'Button';
      props: {
        key: string;
        label: string;
        hotkey?: string;
        action?: string;
        plain?: true;
        dimColor?: boolean;
        variant?: 'primary' | 'secondary';
        role?: 'dismiss';
        autoFocus?: true;
      };
      press: ModUiClientPressRef;
      hover?: ModUiJsonValue;
      children?: readonly [];
    }
  | {
      type: 'Input';
      props: {
        key: string;
        label?: string;
        placeholder?: string;
        value?: string;
        submitLabel?: string;
        autoFocus?: true;
      };
      press: ModUiClientPressRef;
      hover?: ModUiJsonValue;
      children?: readonly [];
    }
  | {
      type: 'Select';
      props: {
        key: string;
        options: readonly SelectOption[];
        value?: string;
        label?: string;
        autoFocus?: true;
      };
      press: ModUiClientPressRef;
      hover?: ModUiJsonValue;
      children?: readonly [];
    }
  | {
      type: 'Link';
      props: { href: string; label?: string };
      hover?: ModUiJsonValue;
      children?: readonly SurfaceChild[];
    }
  | {
      type: 'Code';
      props: {
        source: string;
        language?: string;
        path?: string;
        startLine?: number;
        format?: 'source' | 'diff';
        wrap?: 'wrap' | 'truncate-end';
      };
      hover?: ModUiJsonValue;
      children?: readonly [];
    }
  | {
      type: 'Markdown';
      props: { key?: string; text: string; dimColor?: boolean; pressableLinks?: readonly string[] };
      press?: ModUiClientPressRef;
      hover?: ModUiJsonValue;
      children?: readonly [];
    };

export interface ModUiClientTreeProps {
  tree: unknown;
  site: ModUiRenderSite;
  client: ModUiClientIdentity;
  /** Native ui_client_press request dispatcher. Runtime policy/VM handoff stays outside React. */
  onPress(request: NativeUiClientPressRequest, handle: number): void | Promise<unknown>;
  /** A rejected local VM callback run is reported by the owning host adapter. */
  onRunFault?(request: NativeUiClientPressRequest, reason: string): void;
  /** A render-tree validation failure is reported through the owning host adapter. */
  onRenderFault?(reason: string): void;
  /** Parent Client identities over Native ui_client_press's address bounds are rendered read-only. */
  interactionsEnabled?: boolean;
  renderRevision?: number;
}

const MAX_TREE_NODES = 20_000;
const MAX_TREE_DEPTH = 32;
const MAX_TREE_CHARS = 100_000;
const MAX_DATA_VALUES = 20_000;
const MAX_DATA_DEPTH = 32;
const MAX_REQUEST_STRING = 256;
const MAX_INPUT_VALUE = 16_384;

const BOX_PROP_KEYS = [
  'key', 'flexDirection', 'flexGrow', 'flexShrink', 'flexWrap', 'alignItems', 'alignSelf', 'justifyContent',
  'gap', 'columnGap', 'rowGap', 'width', 'height', 'minWidth', 'minHeight', 'margin', 'marginX',
  'marginY', 'marginTop', 'marginBottom', 'marginLeft', 'marginRight', 'padding', 'paddingX',
  'paddingY', 'paddingTop', 'paddingBottom', 'paddingLeft', 'paddingRight', 'borderStyle',
  'borderColor', 'borderDimColor', 'backgroundColor', 'overflow', 'display', 'position', 'top',
  'left', 'right', 'bottom',
] as const;

const TEXT_PROP_KEYS = [
  'color', 'backgroundColor', 'dimColor', 'bold', 'italic', 'underline', 'strikethrough',
  'inverse', 'wrap', 'key',
] as const;
const BORDER_STYLES = ['single', 'double', 'round', 'bold', 'singleDouble', 'doubleSingle', 'classic', 'arrow', 'dashed', 'quote'] as const;
const FLEX_DIRECTIONS = ['row', 'column', 'row-reverse', 'column-reverse'] as const;
const FLEX_WRAPS = ['nowrap', 'wrap', 'wrap-reverse'] as const;
const ALIGN_ITEMS = ['flex-start', 'center', 'flex-end', 'stretch'] as const;
const ALIGN_SELF = ['flex-start', 'center', 'flex-end', 'auto'] as const;
const JUSTIFY_CONTENT = ['flex-start', 'center', 'flex-end', 'space-between', 'space-around', 'space-evenly'] as const;
const TEXT_WRAPS = ['wrap', 'end', 'middle', 'truncate-end', 'truncate', 'truncate-middle', 'truncate-start'] as const;
const COLOR_VALUE = /^[A-Za-z0-9#(),.%\s_-]{1,40}$/;

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}

function fail(message: string): never {
  throw new TypeError(`Invalid Client surface tree: ${message}`);
}

function validateJson(value: unknown, depth = 0, state = { values: 0 }): asserts value is ModUiJsonValue {
  state.values += 1;
  if (state.values > MAX_DATA_VALUES) fail('JSON payload exceeds the value limit');
  if (depth > MAX_DATA_DEPTH) fail('JSON payload exceeds the depth limit');
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return;
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) fail('JSON numbers must be finite');
    return;
  }
  if (Array.isArray(value)) {
    for (let index = 0; index < value.length; index += 1) {
      if (!(index in value)) fail('sparse arrays are not allowed');
      validateJson(value[index], depth + 1, state);
    }
    return;
  }
  if (!record(value)) fail('values must be plain JSON data');
  for (const entry of Object.values(value)) validateJson(entry, depth + 1, state);
}

function validatePrimitiveRecord(value: unknown, name: string): asserts value is PrimitiveRecord {
  if (!record(value)) fail(`${name} must be an object`);
  for (const [key, entry] of Object.entries(value)) {
    if (key.length === 0 || !['string', 'number', 'boolean'].includes(typeof entry)
      || (typeof entry === 'number' && !Number.isFinite(entry))) {
      fail(`${name} values must be strings, finite numbers, or booleans`);
    }
  }
}

function validateStyleProps(props: Record<string, unknown>, type: 'Box' | 'Text'): void {
  const enums: Record<string, readonly string[]> = type === 'Box'
    ? {
      flexDirection: FLEX_DIRECTIONS,
      flexWrap: FLEX_WRAPS,
      alignItems: ALIGN_ITEMS,
      alignSelf: ALIGN_SELF,
      justifyContent: JUSTIFY_CONTENT,
      borderStyle: BORDER_STYLES,
      overflow: ['visible', 'hidden'],
      display: ['flex', 'none'],
      position: ['relative', 'absolute'],
    }
    : { wrap: TEXT_WRAPS };
  for (const [key, values] of Object.entries(enums)) {
    if (props[key] !== undefined && (typeof props[key] !== 'string' || !values.includes(props[key] as string))) {
      fail(`${type}.props.${key} is invalid`);
    }
  }

  if (props.key !== undefined) validateString(props.key, `${type}.props.key`);
  const booleans = type === 'Box'
    ? ['borderDimColor']
    : ['dimColor', 'bold', 'italic', 'underline', 'strikethrough', 'inverse'];
  for (const key of booleans) {
    if (props[key] !== undefined && typeof props[key] !== 'boolean') fail(`${type}.props.${key} must be boolean`);
  }

  const colors = type === 'Box' ? ['borderColor', 'backgroundColor'] : ['color', 'backgroundColor'];
  for (const key of colors) {
    const value = props[key];
    if (value !== undefined && (typeof value !== 'string' || !COLOR_VALUE.test(value))) fail(`${type}.props.${key} is invalid`);
  }

  if (type === 'Box') {
    for (const key of ['width', 'height', 'minWidth', 'minHeight']) {
      const value = props[key];
      if (value === undefined) continue;
      if (typeof value === 'number') {
        if (!Number.isFinite(value) || value < 0 || value > 10_000) fail(`Box.props.${key} is invalid`);
      } else if (typeof value !== 'string' || !/^\d{1,3}%$/.test(value)) {
        fail(`Box.props.${key} is invalid`);
      }
    }
    for (const key of [
      'flexGrow', 'flexShrink', 'gap', 'columnGap', 'rowGap', 'margin', 'marginX', 'marginY',
      'marginTop', 'marginBottom', 'marginLeft', 'marginRight', 'padding', 'paddingX', 'paddingY',
      'paddingTop', 'paddingBottom', 'paddingLeft', 'paddingRight',
    ]) {
      const value = props[key];
      if (value !== undefined && (typeof value !== 'number' || !Number.isFinite(value) || Math.abs(value) > 10_000)) {
        fail(`Box.props.${key} is invalid`);
      }
    }
    for (const key of ['top', 'left', 'right', 'bottom']) {
      const value = props[key];
      if (value !== undefined && (typeof value !== 'number' || !Number.isSafeInteger(value) || Math.abs(value) > 10_000)) {
        fail(`Box.props.${key} is invalid`);
      }
    }
  }
}

function validateString(value: unknown, name: string, max = MAX_REQUEST_STRING): asserts value is string {
  if (typeof value !== 'string' || value.length === 0 || value.length > max) fail(`${name} is invalid`);
}

function validatePress(value: unknown): asserts value is ModUiClientPressRef {
  if (!record(value)) fail('press must be an object');
  validateString(value.plugin, 'press.plugin');
  if (!Number.isSafeInteger(value.handle) || (value.handle as number) < 1) fail('press.handle must be a positive safe integer');
}

function exactKeys(value: Record<string, unknown>, allowed: readonly string[], required: readonly string[], name: string): void {
  if (Object.keys(value).some((key) => !allowed.includes(key))) fail(`${name} has unsupported properties`);
  if (required.some((key) => !(key in value))) fail(`${name} is missing required properties`);
}

function validateProps(node: Record<string, unknown>, type: ModUiSurfaceNodeType): void {
  const props = node.props;
  if (type === 'Box' || type === 'Text') {
    if (props !== undefined) {
      validatePrimitiveRecord(props, `${type}.props`);
      const allowed = type === 'Box' ? BOX_PROP_KEYS : TEXT_PROP_KEYS;
      const entries = props as Record<string, unknown>;
      if (Object.keys(entries).some((key) => !(allowed as readonly string[]).includes(key))) {
        fail(`${type}.props has unsupported style properties`);
      }
      validateStyleProps(entries, type);
    }
    return;
  }
  if (!record(props)) fail(`${type}.props must be an object`);

  switch (type) {
    case 'Button': {
      exactKeys(props, ['key', 'label', 'hotkey', 'action', 'plain', 'dimColor', 'variant', 'role', 'autoFocus'], ['key', 'label'], type);
      validateString(props.key, 'Button.props.key');
      validateString(props.label, 'Button.props.label', MAX_INPUT_VALUE);
      if (props.hotkey !== undefined && typeof props.hotkey !== 'string') fail('Button.props.hotkey must be a string');
      if (props.action !== undefined && typeof props.action !== 'string') fail('Button.props.action must be a string');
      if (props.plain !== undefined && props.plain !== true) fail('Button.props.plain must be true when set');
      if (props.dimColor !== undefined && typeof props.dimColor !== 'boolean') fail('Button.props.dimColor must be boolean');
      if (props.variant !== undefined && props.variant !== 'primary' && props.variant !== 'secondary') fail('Button.props.variant is invalid');
      if (props.role !== undefined && props.role !== 'dismiss') fail('Button.props.role is invalid');
      if (props.autoFocus !== undefined && props.autoFocus !== true) fail('Button.props.autoFocus must be true when set');
      break;
    }
    case 'Input': {
      exactKeys(props, ['key', 'label', 'placeholder', 'value', 'submitLabel', 'autoFocus'], ['key'], type);
      validateString(props.key, 'Input.props.key');
      for (const key of ['label', 'placeholder', 'value', 'submitLabel'] as const) {
        if (props[key] !== undefined && (typeof props[key] !== 'string' || props[key].length > MAX_INPUT_VALUE)) fail(`Input.props.${key} is invalid`);
      }
      if (props.autoFocus !== undefined && props.autoFocus !== true) fail('Input.props.autoFocus must be true when set');
      break;
    }
    case 'Select': {
      exactKeys(props, ['key', 'options', 'value', 'label', 'autoFocus'], ['key', 'options'], type);
      validateString(props.key, 'Select.props.key');
      if (!Array.isArray(props.options) || props.options.length === 0 || props.options.length > 64) fail('Select.props.options must contain 1 through 64 options');
      const seen = new Set<string>();
      for (const option of props.options) {
        if (!record(option)) fail('Select options must be objects');
        exactKeys(option, ['value', 'label'], ['value'], 'Select option');
        validateString(option.value, 'Select option value', MAX_INPUT_VALUE);
        if (seen.has(option.value)) fail('Select option values must be unique');
        seen.add(option.value);
        if (option.label !== undefined && (typeof option.label !== 'string' || option.label.length > MAX_INPUT_VALUE)) fail('Select option label is invalid');
      }
      if (props.value !== undefined && (typeof props.value !== 'string' || props.value.length > MAX_INPUT_VALUE)) fail('Select.props.value is invalid');
      if (props.label !== undefined && (typeof props.label !== 'string' || props.label.length > MAX_INPUT_VALUE)) fail('Select.props.label is invalid');
      if (props.autoFocus !== undefined && props.autoFocus !== true) fail('Select.props.autoFocus must be true when set');
      break;
    }
    case 'Link':
      exactKeys(props, ['href', 'label'], ['href'], type);
      if (typeof props.href !== 'string' || !props.href.trim() || props.href.length > 2_048) fail('Link.props.href is invalid');
      if (props.label !== undefined && (typeof props.label !== 'string' || !props.label.trim() || props.label.length > 10_000)) fail('Link.props.label is invalid');
      break;
    case 'Code':
      exactKeys(props, ['source', 'language', 'path', 'startLine', 'format', 'wrap'], ['source'], type);
      if (typeof props.source !== 'string' || props.source.length > MAX_TREE_CHARS) fail('Code.props.source is invalid');
      for (const key of ['language', 'path'] as const) {
        if (props[key] !== undefined && (typeof props[key] !== 'string' || props[key].length > MAX_INPUT_VALUE)) fail(`Code.props.${key} is invalid`);
      }
      if (props.startLine !== undefined && (!Number.isSafeInteger(props.startLine) || (props.startLine as number) < 1 || (props.startLine as number) > 1_000_000_000)) fail('Code.props.startLine is invalid');
      if (props.format !== undefined && props.format !== 'source' && props.format !== 'diff') fail('Code.props.format is invalid');
      if (props.wrap !== undefined && props.wrap !== 'wrap' && props.wrap !== 'truncate-end') fail('Code.props.wrap is invalid');
      break;
    case 'Markdown':
      exactKeys(props, ['key', 'text', 'dimColor', 'pressableLinks'], ['text'], type);
      if (props.key !== undefined && (typeof props.key !== 'string' || props.key.length > MAX_REQUEST_STRING)) fail('Markdown.props.key is invalid');
      if (typeof props.text !== 'string' || props.text.length > MAX_TREE_CHARS) fail('Markdown.props.text is invalid');
      if (props.dimColor !== undefined && typeof props.dimColor !== 'boolean') fail('Markdown.props.dimColor must be boolean');
      if (props.pressableLinks !== undefined && (!Array.isArray(props.pressableLinks) || props.pressableLinks.length > 256 || props.pressableLinks.some((value) => typeof value !== 'string' || value.length === 0 || value.length > 2_048))) fail('Markdown.props.pressableLinks is invalid');
      break;
  }
}

function validateNode(value: unknown, depth: number, state: { nodes: number }, expectedPlugin?: string): asserts value is ModUiSurfaceNode {
  if (depth > MAX_TREE_DEPTH) fail('tree exceeds the nesting limit');
  state.nodes += 1;
  if (state.nodes > MAX_TREE_NODES) fail('tree exceeds the node limit');
  if (!record(value)) fail('nodes must be objects');
  const type = value.type;
  if (typeof type !== 'string' || !['Box', 'Text', 'Button', 'Input', 'Select', 'Link', 'Code', 'Markdown'].includes(type)) {
    fail('node type is unsupported');
  }
  const allowed = type === 'Box' || type === 'Text' || type === 'Link'
    ? ['type', 'props', 'hover', 'children']
    : type === 'Markdown' ? ['type', 'props', 'hover', 'children', 'press']
      : type === 'Button' || type === 'Input' || type === 'Select'
        ? ['type', 'props', 'hover', 'children', 'press'] : ['type', 'props', 'hover', 'children'];
  exactKeys(value, allowed, ['type'], type);
  if (value.props !== undefined) validateJson(value.props);
  if (value.hover !== undefined) validateJson(value.hover);
  validateProps(value, type as ModUiSurfaceNodeType);
  if (type === 'Button' || type === 'Input' || type === 'Select' || type === 'Markdown' && value.press !== undefined) {
    validatePress(value.press);
    if (expectedPlugin !== undefined && (value.press as ModUiClientPressRef).plugin !== expectedPlugin) {
      fail('press.plugin must match the owning Client plugin');
    }
  }
  const children = value.children;
  if (children !== undefined) {
    if (!Array.isArray(children)) fail(`${type}.children must be an array`);
    if (['Button', 'Input', 'Select', 'Code', 'Markdown'].includes(type) && children.length > 0) fail(`${type} is a leaf`);
    for (const child of children) {
      if (typeof child === 'string') continue;
      validateNode(child, depth + 1, state, expectedPlugin);
    }
  }
}

/** Validate a host-produced Client surface tree against Native's node bounds and element set. */
export function parseModUiClientTree(value: unknown, expectedPlugin?: string): ModUiSurfaceNode {
  let serialized: string;
  try {
    serialized = JSON.stringify(value);
  } catch {
    fail('tree must be serializable JSON');
  }
  if (serialized === undefined || serialized.length > MAX_TREE_CHARS) fail('tree exceeds the serialized size limit');
  validateNode(value, 0, { nodes: 0 }, expectedPlugin);
  return value;
}

export function buildNativeUiClientPressRequest(
  site: ModUiRenderSite,
  client: ModUiClientIdentity,
  element: string,
  event: ModUiClientPressEvent,
  pressPlugin = client.plugin,
): NativeUiClientPressRequest {
  validateString(site.component, 'site.component');
  validateString(site.instanceId, 'site.instanceId');
  validateString(client.plugin, 'client.plugin');
  validateString(client.key, 'client.key');
  validateString(client.module, 'client.module');
  validateString(element, 'element');
  validateString(pressPlugin, 'press.plugin');
  if (pressPlugin !== client.plugin) fail('press.plugin must match the owning Client plugin');
  if (event.type === 'input') {
    if (event.kind !== 'change' && event.kind !== 'submit') fail('input event kind is invalid');
    if (typeof event.value !== 'string' || event.value.length > MAX_INPUT_VALUE) fail('input event value is invalid');
  } else if (event.type === 'select') {
    if (typeof event.value !== 'string' || event.value.length > MAX_INPUT_VALUE) fail('select event value is invalid');
  } else if (event.type !== 'press') {
    fail('press event is invalid');
  }
  return {
    subtype: 'ui_client_press',
    plugin: client.plugin,
    component: site.component,
    instance_id: site.instanceId,
    client: client.key,
    module: client.module,
    element,
    event,
  };
}

class ClientSurfaceErrorBoundary extends Component<{
  children: ReactNode;
  onFault?(reason: string): void;
}, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError(): { failed: boolean } {
    return { failed: true };
  }

  componentDidCatch(error: Error, _info: ErrorInfo): void {
    this.props.onFault?.(error.message);
  }

  render(): ReactNode {
    if (this.state.failed) {
      return <div className="mod-ui-client-fallback" role="status">This Mod surface is unavailable.</div>;
    }
    return this.props.children;
  }
}

/** React renderer for the bounded JSON tree returned by the Harness Node VM. It never imports or evaluates Mod source. */
export const ModUiClientTree = memo(function ModUiClientTree({ tree, site, client, onPress, onRunFault, onRenderFault, interactionsEnabled = true, renderRevision }: ModUiClientTreeProps) {
  const body = React.createElement(ModUiClientTreeBody, { tree, site, client, onPress, onRunFault, interactionsEnabled, renderRevision });
  return React.createElement(ClientSurfaceErrorBoundary, {
    key: `${client.plugin}\u0000${client.key}\u0000${client.module}\u0000${renderRevision ?? 0}`,
    onFault: onRenderFault,
    children: body,
  });
});

const ModUiClientTreeBody = memo(function ModUiClientTreeBody({ tree, site, client, onPress, onRunFault, interactionsEnabled = true, renderRevision }: Omit<ModUiClientTreeProps, 'onRenderFault'>) {
  const parsed = parseModUiClientTree(tree, client.plugin);
  const lifecycle = useRef(0);
  const onPressRef = useRef(onPress);
  const onRunFaultRef = useRef(onRunFault);
  onPressRef.current = onPress;
  onRunFaultRef.current = onRunFault;

  useEffect(() => {
    const activeGeneration = ++lifecycle.current;
    return () => {
      if (lifecycle.current === activeGeneration) lifecycle.current += 1;
    };
  }, [site.component, site.instanceId, client.plugin, client.key, client.module, renderRevision]);

  const dispatch = useCallback((press: ModUiClientPressRef, element: string, event: ModUiClientPressEvent) => {
    if (!interactionsEnabled) return;
    const request = buildNativeUiClientPressRequest(site, client, element, event, press.plugin);
    const activeGeneration = lifecycle.current;
    try {
      void Promise.resolve(onPressRef.current(request, press.handle)).catch((error: unknown) => {
        if (lifecycle.current !== activeGeneration) return;
        const reason = error instanceof Error ? error.message : String(error);
        onRunFaultRef.current?.(request, reason);
      });
    } catch (error) {
      if (lifecycle.current !== activeGeneration) return;
      const reason = error instanceof Error ? error.message : String(error);
      onRunFaultRef.current?.(request, reason);
    }
  }, [site.component, site.instanceId, client.plugin, client.key, client.module, interactionsEnabled]);

  return <div className="mod-ui-client-surface" data-mod-client={client.key} data-mod-plugin={client.plugin} data-mod-module={client.module}>
    <SurfaceNode node={parsed} dispatch={dispatch} interactionsEnabled={interactionsEnabled} />
  </div>;
});

const SurfaceNode = memo(function SurfaceNode({ node, dispatch, interactionsEnabled }: {
  node: ModUiSurfaceNode;
  dispatch(press: ModUiClientPressRef, element: string, event: ModUiClientPressEvent): void;
  interactionsEnabled: boolean;
}): ReactElement {
  const t = useT();
  const children = node.children?.map((child, index) => typeof child === 'string'
    ? child
    : <SurfaceNode key={childKey(child, index)} node={child} dispatch={dispatch} interactionsEnabled={interactionsEnabled} />);
  const style = node.type === 'Box' ? modUiBoxStyle(node.props)
    : node.type === 'Text' ? modUiTextStyle(node.props) : undefined;

  switch (node.type) {
    case 'Box':
      return <div className="mod-ui-client-box" style={style}>{children}</div>;
    case 'Text':
      return <span className="mod-ui-client-text" style={style}>{children}</span>;
    case 'Button': {
      const { key, label, hotkey, action, plain, dimColor, variant, role, autoFocus } = node.props;
      const buttonStyle: CSSProperties = {
        border: plain ? 0 : `1px solid ${t.border}`,
        borderRadius: 7,
        background: plain ? 'transparent' : variant === 'primary' ? t.accent : t.surface,
        color: dimColor ? t.text3 : variant === 'primary' ? '#fff' : t.text,
        cursor: 'pointer',
        font: 'inherit',
        padding: plain ? '3px 6px' : '6px 10px',
        opacity: dimColor ? 0.72 : 1,
      };
      return <button type="button" className="mod-ui-client-button" style={buttonStyle} autoFocus={autoFocus} disabled={!interactionsEnabled} aria-keyshortcuts={hotkey} aria-label={role === 'dismiss' ? label : undefined} data-action={action} data-role={role} onClick={() => dispatch(node.press, key, { type: 'press' })}>{label}</button>;
    }
    case 'Input':
      return <SurfaceInput node={node} dispatch={dispatch} disabled={!interactionsEnabled} />;
    case 'Select':
      return <SurfaceSelect node={node} dispatch={dispatch} disabled={!interactionsEnabled} />;
    case 'Link': {
      const href = safeLink(node.props.href);
      const content = children?.length ? children : node.props.label ?? node.props.href;
      return href ? <a className="mod-ui-client-link" href={href} target="_blank" rel="noreferrer" style={{ color: t.link ?? t.accent2, textDecoration: 'underline', textUnderlineOffset: 2 }}>{content}</a>
        : <span className="mod-ui-client-link-disabled" style={{ color: t.text3 }}>{content}</span>;
    }
    case 'Code':
      return <figure className="mod-ui-client-code" data-path={node.props.path} data-start-line={node.props.startLine} data-format={node.props.format} data-wrap={node.props.wrap} style={{ minWidth: 0, margin: 0 }}>
        {node.props.path && <figcaption style={{ marginBottom: 5, color: t.text3, fontSize: 12 }}>{node.props.path}</figcaption>}
        <CodeBlock code={node.props.source} language={node.props.language} variant="tool" />
      </figure>;
    case 'Markdown':
      return <div className="mod-ui-client-markdown" data-pressable-links={node.props.pressableLinks?.join(' ')} style={node.props.dimColor ? { color: t.text3 } : undefined}>
        <MarkdownContent text={node.props.text} />
      </div>;
  }
});

const SurfaceInput = memo(function SurfaceInput({ node, dispatch, disabled }: {
  node: Extract<ModUiSurfaceNode, { type: 'Input' }>;
  dispatch(press: ModUiClientPressRef, element: string, event: ModUiClientPressEvent): void;
  disabled: boolean;
}) {
  const t = useT();
  const { key, label, placeholder, value, submitLabel, autoFocus } = node.props;
  const [draft, setDraft] = useState(value ?? '');
  useEffect(() => setDraft(value ?? ''), [key, value]);
  const submit = useCallback((event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const input = event.currentTarget.querySelector('input');
    dispatch(node.press, key, { type: 'input', kind: 'submit', value: input?.value ?? draft });
  }, [dispatch, draft, key, node.press]);
  const onChange = useCallback((event: ChangeEvent<HTMLInputElement>) => {
    const nextValue = event.currentTarget.value;
    setDraft(nextValue);
    dispatch(node.press, key, { type: 'input', kind: 'change', value: nextValue });
  }, [dispatch, key, node.press]);
  const submitOnEnter = useCallback((event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== 'Enter' || event.nativeEvent.isComposing) return;
    event.preventDefault();
    dispatch(node.press, key, { type: 'input', kind: 'submit', value: event.currentTarget.value });
  }, [dispatch, key, node.press]);
  return <form className="mod-ui-client-input-row" onSubmit={submit} style={{ display: 'flex', gap: 8, minWidth: 0 }}>
    <label className="mod-ui-client-input-label" style={{ display: 'grid', flex: 1, minWidth: 0, gap: 4, color: t.text2 }}>
      {label && <span>{label}</span>}
      <input className="mod-ui-client-input" type="text" value={draft} placeholder={placeholder} autoFocus={autoFocus} disabled={disabled} maxLength={MAX_INPUT_VALUE} onChange={onChange} onKeyDown={submitOnEnter} style={{ minWidth: 0, border: `1px solid ${t.border}`, borderRadius: 7, padding: '7px 9px', background: t.windowBg, color: t.text, font: 'inherit' }} />
    </label>
    {submitLabel && <button type="submit" className="mod-ui-client-input-submit" disabled={disabled} style={{ alignSelf: 'end', border: `1px solid ${t.border}`, borderRadius: 7, padding: '7px 10px', background: t.surface, color: t.text, font: 'inherit', cursor: disabled ? 'not-allowed' : 'pointer' }}>{submitLabel}</button>}
  </form>;
});

const SurfaceSelect = memo(function SurfaceSelect({ node, dispatch, disabled }: {
  node: Extract<ModUiSurfaceNode, { type: 'Select' }>;
  dispatch(press: ModUiClientPressRef, element: string, event: ModUiClientPressEvent): void;
  disabled: boolean;
}) {
  const t = useT();
  const { key, options, value, label, autoFocus } = node.props;
  const defaultValue = value ?? options[0]?.value ?? '';
  const [selected, setSelected] = useState(defaultValue);
  useEffect(() => setSelected(defaultValue), [key, defaultValue]);
  const onChange = useCallback((event: ChangeEvent<HTMLSelectElement>) => {
    const nextValue = event.currentTarget.value;
    setSelected(nextValue);
    dispatch(node.press, key, { type: 'select', value: nextValue });
  }, [dispatch, key, node.press]);
  return <label className="mod-ui-client-select-label" style={{ display: 'grid', gap: 4, color: t.text2 }}>
    {label && <span>{label}</span>}
    <select className="mod-ui-client-select" value={selected} autoFocus={autoFocus} disabled={disabled} onChange={onChange} style={{ minWidth: 0, border: `1px solid ${t.border}`, borderRadius: 7, padding: '7px 9px', background: t.windowBg, color: t.text, font: 'inherit' }}>
      {options.map((option) => <option key={option.value} value={option.value}>{option.label ?? option.value}</option>)}
    </select>
  </label>;
});

function childKey(node: ModUiSurfaceNode, index: number): string {
  if (node.type === 'Button' || node.type === 'Input' || node.type === 'Select') return `${node.type}:${node.props.key}`;
  if (node.type === 'Markdown' && node.props.key) return `Markdown:${node.props.key}`;
  return `${node.type}:${index}`;
}

export function modUiBoxStyle(props?: PrimitiveRecord): CSSProperties {
  const style: Record<string, string | number> = { display: 'flex', flexDirection: 'column', minWidth: 0 };
  if (!props) return style as CSSProperties;
  for (const key of ['flexDirection', 'flexGrow', 'flexShrink', 'flexWrap', 'alignItems', 'alignSelf', 'justifyContent', 'gap', 'columnGap', 'rowGap', 'width', 'height', 'minWidth', 'minHeight', 'margin', 'marginTop', 'marginBottom', 'marginLeft', 'marginRight', 'padding', 'paddingTop', 'paddingBottom', 'paddingLeft', 'paddingRight', 'borderColor', 'backgroundColor', 'overflow', 'display', 'position', 'top', 'left', 'right', 'bottom'] as const) {
    const value = props[key];
    if (typeof value === 'string' || typeof value === 'number') style[key] = value;
  }
  if (typeof props.marginX === 'number') style.marginInline = props.marginX;
  if (typeof props.marginY === 'number') style.marginBlock = props.marginY;
  if (typeof props.paddingX === 'number') style.paddingInline = props.paddingX;
  if (typeof props.paddingY === 'number') style.paddingBlock = props.paddingY;
  if (typeof props.borderStyle === 'string') {
    style.borderStyle = props.borderStyle === 'double' ? 'double'
      : props.borderStyle === 'dashed' ? 'dashed' : 'solid';
  }
  if (props.borderDimColor === true) style.opacity = 0.72;
  return style as CSSProperties;
}

export function modUiTextStyle(props?: PrimitiveRecord): CSSProperties {
  const style: Record<string, string | number> = { minWidth: 0, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' };
  if (!props) return style as CSSProperties;
  if (typeof props.color === 'string') style.color = props.color;
  if (typeof props.backgroundColor === 'string') style.backgroundColor = props.backgroundColor;
  if (props.bold === true) style.fontWeight = 600;
  if (props.italic === true) style.fontStyle = 'italic';
  if (props.dimColor === true) style.opacity = 0.72;
  if (props.inverse === true) {
    style.color = 'var(--mod-ui-inverse-fg, Canvas)';
    style.backgroundColor = 'var(--mod-ui-inverse-bg, CanvasText)';
  }
  if (props.underline === true || props.strikethrough === true) {
    style.textDecoration = [props.underline === true ? 'underline' : '', props.strikethrough === true ? 'line-through' : ''].filter(Boolean).join(' ');
  }
  if (props.wrap === 'truncate' || props.wrap === 'truncate-end' || props.wrap === 'end') {
    style.whiteSpace = 'nowrap';
    style.textOverflow = 'ellipsis';
    style.overflow = 'hidden';
  } else if (props.wrap === 'wrap') {
    style.whiteSpace = 'pre-wrap';
  } else if (props.wrap === 'truncate-start') {
    style.whiteSpace = 'nowrap';
    style.textOverflow = 'ellipsis';
    style.overflow = 'hidden';
    style.direction = 'rtl';
    style.textAlign = 'left';
  } else if (props.wrap === 'truncate-middle' || props.wrap === 'middle') {
    style.whiteSpace = 'nowrap';
    style.textOverflow = 'ellipsis';
    style.overflow = 'hidden';
  }
  return style as CSSProperties;
}

function safeLink(value: string): string | undefined {
  try {
    if (value.length > 2_048 || /[^\x20-\x7e]/.test(value) || value.includes('@')) return undefined;
    const url = new URL(value);
    if (url.username || url.password) return undefined;
    if (url.protocol === 'https:') return url.href.length <= 2_048 ? url.href : undefined;
    if (url.protocol === 'http:' && url.hostname === 'localhost') return url.href.length <= 2_048 ? url.href : undefined;
  } catch {
    // Surface links are untrusted display data; unsupported and relative URLs stay inert.
  }
  return undefined;
}
