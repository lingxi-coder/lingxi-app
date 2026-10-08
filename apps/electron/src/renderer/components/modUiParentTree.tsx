import * as React from 'react';
import {
  createContext,
  memo,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useState,
  type CSSProperties,
  type FormEvent,
  type ReactNode,
} from 'react';
import type {
  NativeUiComponent,
  NativeUiParentControlRequest,
  NativeUiSurfaceDto,
  UiJsonValue,
} from '@lingxi/bridge-client';
import { useT } from '../theme/ThemeContext';
import { highlightCodeForDisplay } from './CodeBlock';
import { MarkdownContent } from './MarkdownContent';
import { modUiBoxStyle, modUiTextStyle, type ModUiClientIdentity } from './modUiClientTree';

export interface ModUiParentClientDescriptor extends ModUiClientIdentity {
  props: Record<string, UiJsonValue>;
  width?: string | number;
  height?: string | number;
  flexGrow?: number;
}

export interface ModUiParentSiteIdentity {
  surface: NativeUiSurfaceDto;
  component: NativeUiComponent;
  instanceId: string;
}

export interface ModUiParentEngineFallbackContext {
  ref: number;
  requestProps: Record<string, UiJsonValue>;
  responseProps: Record<string, UiJsonValue>;
}

export type ModUiParentEngineFallback = ReactNode | ((context: ModUiParentEngineFallbackContext) => ReactNode);

export interface ModUiParentTreeProps {
  tree: UiJsonValue | null;
  site: ModUiParentSiteIdentity;
  requestProps: Record<string, UiJsonValue>;
  responseProps: Record<string, UiJsonValue>;
  fallback?: ReactNode;
  engineFallback?: ModUiParentEngineFallback;
  renderClient(descriptor: ModUiParentClientDescriptor, reactKey: string): ReactNode;
  onParentControl(request: NativeUiParentControlRequest): void | Promise<unknown>;
}

type ParentRecord = Record<string, unknown>;
type ParentNode = ParentRecord & { type: string };
type ParentPress = { plugin: string; handle: number };
type ParentHover = ParentRecord;

const BOX_STYLE_KEYS = [
  'key', 'flexDirection', 'flexGrow', 'flexShrink', 'flexWrap', 'alignItems', 'alignSelf', 'justifyContent', 'gap', 'columnGap', 'rowGap',
  'width', 'height', 'minWidth', 'minHeight', 'margin', 'marginX', 'marginY', 'marginTop', 'marginBottom', 'marginLeft', 'marginRight',
  'padding', 'paddingX', 'paddingY', 'paddingTop', 'paddingBottom', 'paddingLeft', 'paddingRight', 'borderStyle', 'borderColor',
  'borderDimColor', 'backgroundColor', 'overflow', 'display', 'position', 'top', 'left', 'right', 'bottom',
] as const;
const TEXT_STYLE_KEYS = ['key', 'color', 'backgroundColor', 'dimColor', 'bold', 'italic', 'underline', 'strikethrough', 'inverse', 'wrap'] as const;
const PARENT_COLORS = /^[#a-zA-Z0-9_().,% -]{1,40}$/;
const MAX_PARENT_TREE_DEPTH = 32;
const MAX_PARENT_TREE_NODES = 20_000;
const MAX_PARENT_TREE_UTF16 = 100_000;
const MAX_PARENT_DATA_VALUES = 20_000;
const MAX_PARENT_DATA_UTF16 = 100_000;
const MAX_PARENT_SAFE_INTEGER = Number.MAX_SAFE_INTEGER;

interface ParentHoverContextValue {
  activeGroup: string | null;
  setActiveGroup(group: string | null): void;
}

const ParentHoverContext = createContext<ParentHoverContextValue>({ activeGroup: null, setActiveGroup: () => undefined });

function isRecord(value: unknown): value is ParentRecord {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}

function assertKeys(value: ParentRecord, allowed: readonly string[], name: string): void {
  if (Object.keys(value).some((key) => !allowed.includes(key))) throw new TypeError(`${name} contains unsupported fields`);
}

function requiredRecord(value: unknown, name: string): ParentRecord {
  if (!isRecord(value)) throw new TypeError(`${name} must be an object`);
  return value;
}

function requiredString(value: unknown, name: string): string {
  if (typeof value !== 'string') throw new TypeError(`${name} must be a string`);
  return value;
}

function utf16Length(value: string): number {
  return value.length;
}

function validateParentData(value: unknown, name: string): void {
  let count = 0;
  const visit = (current: unknown, depth: number): void => {
    count += 1;
    if (count > MAX_PARENT_DATA_VALUES || depth > MAX_PARENT_TREE_DEPTH) {
      throw new TypeError(`${name} exceeds the Native plain-data bounds`);
    }
    if (current === null || typeof current === 'string' || typeof current === 'boolean') return;
    if (typeof current === 'number') {
      if (!Number.isFinite(current)) throw new TypeError(`${name} contains a number that is not finite`);
      return;
    }
    if (Array.isArray(current)) {
      for (let index = 0; index < current.length; index += 1) {
        if (!(index in current)) throw new TypeError(`${name} cannot contain sparse arrays`);
        visit(current[index], depth + 1);
      }
      return;
    }
    if (!isRecord(current)) throw new TypeError(`${name} must be plain JSON data`);
    for (const child of Object.values(current)) visit(child, depth + 1);
  };
  visit(value, 0);
  let serialized: string;
  try {
    serialized = JSON.stringify(value);
  } catch {
    throw new TypeError(`${name} must be serializable JSON data`);
  }
  if (serialized === undefined || utf16Length(serialized) > MAX_PARENT_DATA_UTF16) {
    throw new TypeError(`${name} exceeds the Native serialized-data limit`);
  }
}

function nonEmptyString(value: unknown, name: string): string {
  const string = requiredString(value, name);
  if (string.length === 0) throw new TypeError(`${name} must be non-empty`);
  return string;
}

function parentPress(value: unknown, name: string): ParentPress {
  const press = requiredRecord(value, `${name}.press`);
  assertKeys(press, ['plugin', 'handle'], `${name}.press`);
  const plugin = nonEmptyString(press.plugin, `${name}.press.plugin`);
  if (typeof press.handle !== 'number' || !Number.isSafeInteger(press.handle) || press.handle < 1) {
    throw new TypeError(`${name}.press.handle must be a positive safe integer`);
  }
  return { plugin, handle: press.handle };
}

function optionalString(props: ParentRecord, key: string, name: string): void {
  if (props[key] !== undefined && typeof props[key] !== 'string') throw new TypeError(`${name}.${key} must be a string`);
}

function optionalTrue(props: ParentRecord, key: string, name: string): void {
  if (props[key] !== undefined && props[key] !== true) throw new TypeError(`${name}.${key} must be true when set`);
}

function validateStyleProps(type: 'Box' | 'Text', props: ParentRecord): void {
  const keys = type === 'Box' ? BOX_STYLE_KEYS : TEXT_STYLE_KEYS;
  assertKeys(props, keys, `${type}.props`);
  if (Object.values(props).some((value) => value !== null && !['string', 'number', 'boolean'].includes(typeof value))) {
    throw new TypeError(`${type}.props values must be primitive`);
  }
  if (props.key !== undefined && (typeof props.key !== 'string' || props.key.length === 0)) throw new TypeError(`${type}.props.key must be non-empty`);
  if (type === 'Box') {
    const enums: Record<string, readonly string[]> = {
      flexDirection: ['row', 'column', 'row-reverse', 'column-reverse'],
      flexWrap: ['nowrap', 'wrap', 'wrap-reverse'],
      alignItems: ['flex-start', 'center', 'flex-end', 'stretch'],
      alignSelf: ['flex-start', 'center', 'flex-end', 'auto'],
      justifyContent: ['flex-start', 'center', 'flex-end', 'space-between', 'space-around', 'space-evenly'],
      borderStyle: ['single', 'double', 'round', 'bold', 'singleDouble', 'doubleSingle', 'classic', 'arrow', 'dashed', 'quote'],
      overflow: ['visible', 'hidden'],
      display: ['flex', 'none'],
      position: ['relative', 'absolute'],
    };
    for (const [key, values] of Object.entries(enums)) {
      if (props[key] !== undefined && (typeof props[key] !== 'string' || !values.includes(props[key] as string))) throw new TypeError(`Box.props.${key} is invalid`);
    }
    for (const key of ['width', 'height', 'minWidth', 'minHeight']) {
      const value = props[key];
      if (value === undefined) continue;
      if (typeof value === 'number') {
        if (!Number.isFinite(value) || value < 0 || value > 10_000) throw new TypeError(`Box.props.${key} is invalid`);
      } else if (typeof value !== 'string' || !/^\d{1,3}%$/.test(value)) throw new TypeError(`Box.props.${key} is invalid`);
    }
    for (const key of [
      'flexGrow', 'flexShrink', 'gap', 'columnGap', 'rowGap', 'margin', 'marginX', 'marginY', 'marginTop', 'marginBottom', 'marginLeft', 'marginRight',
      'padding', 'paddingX', 'paddingY', 'paddingTop', 'paddingBottom', 'paddingLeft', 'paddingRight',
    ]) {
      const value = props[key];
      if (value !== undefined && (typeof value !== 'number' || !Number.isFinite(value) || Math.abs(value) > 10_000)) throw new TypeError(`Box.props.${key} is invalid`);
    }
    for (const key of ['top', 'left', 'right', 'bottom']) {
      const value = props[key];
      if (value !== undefined && (typeof value !== 'number' || !Number.isInteger(value) || Math.abs(value) > 10_000)) throw new TypeError(`Box.props.${key} is invalid`);
    }
    for (const key of ['borderColor', 'backgroundColor']) {
      const value = props[key];
      if (value !== undefined && (typeof value !== 'string' || !PARENT_COLORS.test(value))) throw new TypeError(`Box.props.${key} is invalid`);
    }
    if (props.borderDimColor !== undefined && typeof props.borderDimColor !== 'boolean') throw new TypeError('Box.props.borderDimColor is invalid');
    return;
  }
  const colors = ['color', 'backgroundColor'];
  for (const key of colors) {
    const value = props[key];
    if (value !== undefined && (typeof value !== 'string' || !PARENT_COLORS.test(value))) throw new TypeError(`Text.props.${key} is invalid`);
  }
  for (const key of ['dimColor', 'bold', 'italic', 'underline', 'strikethrough', 'inverse']) {
    if (props[key] !== undefined && typeof props[key] !== 'boolean') throw new TypeError(`Text.props.${key} is invalid`);
  }
  if (props.wrap !== undefined && (typeof props.wrap !== 'string'
    || !['wrap', 'end', 'middle', 'truncate-end', 'truncate', 'truncate-middle', 'truncate-start'].includes(props.wrap))) {
    throw new TypeError('Text.props.wrap is invalid');
  }
}

function validateParentGroup(value: unknown): void {
  const group = requiredRecord(value, 'group');
  assertKeys(group, ['plugin'], 'group');
  const plugin = nonEmptyString(group.plugin, 'group.plugin');
  if (utf16Length(plugin) > 256) throw new TypeError('group.plugin exceeds the Native limit');
}

function validateParentHover(
  type: 'Box' | 'Text' | 'Button',
  hover: unknown,
  props: ParentRecord,
  node: ParentRecord,
  hasKeyedBoxScope: boolean,
): ParentHover {
  const value = requiredRecord(hover, `${type}.hover`);
  const allowed = type === 'Box'
    ? ['scope', 'borderStyle', 'borderColor', 'borderDimColor', 'backgroundColor', 'display', 'top', 'left', 'right', 'bottom']
    : ['scope', 'color', 'backgroundColor', 'dimColor', 'bold', 'italic', 'underline', 'strikethrough', 'inverse'];
  assertKeys(value, allowed, `${type}.hover`);
  if (value.scope !== undefined) {
    const scope = nonEmptyString(value.scope, `${type}.hover.scope`);
    if (utf16Length(scope) > 64 || /[\u0000-\u001f\u007f-\u009f]/.test(scope)) {
      throw new TypeError(`${type}.hover.scope is invalid`);
    }
  } else if (!hasKeyedBoxScope) {
    throw new TypeError(`${type}.hover requires a keyed Box scope`);
  }
  if (Object.values(value).some((entry) => entry === null || Array.isArray(entry) || isRecord(entry))) {
    throw new TypeError(`${type}.hover values must be primitive`);
  }
  for (const key of ['color', 'borderColor', 'backgroundColor']) {
    const entry = value[key];
    if (entry !== undefined && (typeof entry !== 'string' || !PARENT_COLORS.test(entry))) {
      throw new TypeError(`${type}.hover.${key} is invalid`);
    }
  }
  for (const key of ['borderDimColor', 'dimColor', 'bold', 'italic', 'underline', 'strikethrough', 'inverse']) {
    const entry = value[key];
    if (entry !== undefined && typeof entry !== 'boolean') throw new TypeError(`${type}.hover.${key} must be boolean`);
  }
  if (type === 'Box') {
    const enums: Record<string, readonly string[]> = {
      borderStyle: ['single', 'double', 'round', 'bold', 'singleDouble', 'doubleSingle', 'classic', 'arrow', 'dashed', 'quote'],
      display: ['flex'],
    };
    for (const [key, values] of Object.entries(enums)) {
      if (value[key] !== undefined && (typeof value[key] !== 'string' || !values.includes(value[key] as string))) {
        throw new TypeError(`Box.hover.${key} is invalid`);
      }
    }
    if (value.borderStyle !== undefined && props.borderStyle === undefined) {
      throw new TypeError('Box.hover.borderStyle requires a base border');
    }
    if (value.display !== undefined && props.display !== 'none') {
      throw new TypeError('Box.hover.display requires a hidden base Box');
    }
    for (const key of ['top', 'left', 'right', 'bottom']) {
      const entry = value[key];
      if (entry !== undefined && (props.position !== 'absolute' || typeof entry !== 'number'
        || !Number.isSafeInteger(entry) || Math.abs(entry) > 10_000)) {
        throw new TypeError(`Box.hover.${key} requires an absolute position and bounded integer`);
      }
    }
  }
  if (value.scope !== undefined) {
    if (type === 'Button') parentPress(node.press, type);
    else {
      if (node.group === undefined) throw new TypeError(`${type} scoped hover needs a group owner`);
      validateParentGroup(node.group);
    }
  }
  return value;
}

function validateClientElement(value: ParentRecord): ModUiParentClientDescriptor {
  const props = requiredRecord(value.props, 'Client.props');
  const stamp = requiredRecord(value.client, 'Client.client');
  if (value.children !== undefined) throw new TypeError('Client elements are leaves and cannot have children');
  assertKeys(value, ['type', 'props', 'client'], 'Client');
  assertKeys(props, ['key', 'module', 'props', 'width', 'height', 'flexGrow'], 'Client.props');
  assertKeys(stamp, ['plugin'], 'Client.client');
  const key = nonEmptyString(props.key, 'Client.props.key');
  const module = nonEmptyString(props.module, 'Client.props.module');
  const plugin = nonEmptyString(stamp.plugin, 'Client.client.plugin');
  if (utf16Length(key) > 10_000 || utf16Length(module) > 10_000 || utf16Length(plugin) > 256) {
    throw new TypeError('Client identity exceeds the Native bounds');
  }
  const clientProps = props.props === undefined ? {} : props.props;
  if (!isRecord(clientProps)) throw new TypeError('Client.props.props must be an object');
  validateParentData(clientProps, 'Client.props.props');
  for (const field of ['width', 'height'] as const) {
    const dimension = props[field];
    if (dimension === undefined) continue;
    if (typeof dimension === 'number') {
      if (!Number.isFinite(dimension) || dimension < 0 || dimension > 10_000) throw new TypeError(`Client.props.${field} is invalid`);
    } else if (typeof dimension !== 'string' || !/^\d{1,3}%$/.test(dimension)) {
      throw new TypeError(`Client.props.${field} is invalid`);
    }
  }
  if (props.flexGrow !== undefined && (typeof props.flexGrow !== 'number' || !Number.isFinite(props.flexGrow) || Math.abs(props.flexGrow) > 10_000)) {
    throw new TypeError('Client.props.flexGrow is invalid');
  }
  return {
    plugin,
    key,
    module,
    props: clientProps as Record<string, UiJsonValue>,
    ...(props.width === undefined ? {} : { width: props.width as string | number }),
    ...(props.height === undefined ? {} : { height: props.height as string | number }),
    ...(props.flexGrow === undefined ? {} : { flexGrow: props.flexGrow as number }),
  };
}

interface ParentTreeBudget { nodes: number }

function validateParentNode(
  value: unknown,
  parentType: string | undefined,
  budget: ParentTreeBudget,
  depth: number,
  inheritedHoverScope: boolean,
): asserts value is ParentNode | string {
  if (depth > MAX_PARENT_TREE_DEPTH) throw new TypeError('Parent UI tree exceeds the Native nesting limit');
  if (typeof value === 'string') {
    if (utf16Length(value) > 10_000) throw new TypeError('Parent UI text exceeds the Native limit');
    return;
  }
  budget.nodes += 1;
  if (budget.nodes > MAX_PARENT_TREE_NODES) throw new TypeError('Parent UI tree exceeds the Native node limit');
  if (!isRecord(value) || typeof value.type !== 'string') throw new TypeError('Parent UI children must be text or element objects');
  const node = value as ParentNode;
  const type = node.type;
  if (!['Box', 'Text', 'div', 'span', 'b', 'Button', 'Input', 'Select', 'Link', 'Code', 'Markdown', 'Client', 'Svg', 'engine'].includes(type)) {
    throw new TypeError(`Unsupported Native Parent UI element: ${type}`);
  }
  if (type === 'engine') {
    assertKeys(node, ['type', 'ref'], 'engine');
    if (typeof node.ref !== 'number' || !Number.isSafeInteger(node.ref) || Math.abs(node.ref) > MAX_PARENT_SAFE_INTEGER) {
      throw new TypeError('engine.ref must be a safe integer');
    }
    return;
  }
  if (type === 'Client') {
    validateClientElement(node);
    if (parentType === 'Client') throw new TypeError('Client elements are leaves');
    return;
  }

  const structural = type === 'Box' || type === 'Text' || type === 'div' || type === 'span' || type === 'b';
  const props = structural && node.props === undefined ? {} : requiredRecord(node.props, `${type}.props`);
  validateParentData(props, `${type}.props`);

  const keyedBoxScope = inheritedHoverScope || (type === 'Box' && typeof props.key === 'string' && props.key.length > 0);
  let hover: ParentHover | undefined;
  if (node.hover !== undefined) {
    if (type !== 'Box' && type !== 'Text' && type !== 'Button') throw new TypeError(`${type} cannot carry hover styles`);
    hover = validateParentHover(type, node.hover, props, node, keyedBoxScope);
  }
  if (type === 'Box' && typeof props.key === 'string' && props.key.length > 0 && props.display === 'none'
    && hover?.display !== 'flex' && subtreeHasParentHover(node)) {
    throw new TypeError('A hidden keyed Box must become visible while its hover scope is active');
  }

  const childTypes = structural || type === 'Link';
  if (node.children !== undefined) {
    if (!childTypes || !Array.isArray(node.children)) throw new TypeError(`${type} does not accept children`);
    for (const child of node.children) {
      if ((type === 'Text' || type === 'Link') && typeof child !== 'string') {
        throw new TypeError(`${type} children must be strings`);
      }
      validateParentNode(child, type, budget, depth + 1, keyedBoxScope);
    }
  }

  switch (type) {
    case 'Box':
    case 'Text':
      assertKeys(node, ['type', 'props', 'children', 'hover', 'group'], type);
      validateStyleProps(type, props);
      if (node.group !== undefined) validateParentGroup(node.group);
      break;
    case 'div':
    case 'span':
    case 'b':
      assertKeys(node, ['type', 'props', 'children', 'hover', 'group'], type);
      if (Object.values(props).some((entry) => entry === null || !['string', 'number', 'boolean'].includes(typeof entry))) {
        throw new TypeError(`${type}.props values must be strings, numbers, or booleans`);
      }
      if (node.group !== undefined) validateParentGroup(node.group);
      break;
    case 'Button': {
      assertKeys(node, ['type', 'props', 'press', 'hover'], type);
      assertKeys(props, ['key', 'label', 'hotkey', 'action', 'plain', 'dimColor', 'variant', 'role', 'autoFocus'], `${type}.props`);
      nonEmptyString(props.key, `${type}.props.key`);
      requiredString(props.label, `${type}.props.label`);
      if (props.hotkey !== undefined && (typeof props.hotkey !== 'string' || props.hotkey.length !== 1
        || !/^[a-z0-9]$/.test(props.hotkey))) throw new TypeError('Button.props.hotkey is invalid');
      optionalString(props, 'action', type);
      if (props.action === '') throw new TypeError('Button.props.action must be non-empty');
      if (props.plain !== undefined && props.plain !== true) throw new TypeError('Button.props.plain must be true');
      for (const key of ['dimColor']) if (props[key] !== undefined && typeof props[key] !== 'boolean') throw new TypeError(`Button.props.${key} must be boolean`);
      if (props.variant !== undefined && props.variant !== 'primary' && props.variant !== 'secondary') throw new TypeError('Button.props.variant is invalid');
      if (props.role !== undefined && props.role !== 'dismiss') throw new TypeError('Button.props.role is invalid');
      optionalTrue(props, 'autoFocus', type);
      parentPress(node.press, type);
      break;
    }
    case 'Input':
      assertKeys(node, ['type', 'props', 'press'], type);
      assertKeys(props, ['key', 'label', 'placeholder', 'value', 'submitLabel', 'autoFocus'], `${type}.props`);
      nonEmptyString(props.key, `${type}.props.key`);
      for (const key of ['label', 'placeholder', 'value', 'submitLabel']) optionalString(props, key, type);
      optionalTrue(props, 'autoFocus', type);
      parentPress(node.press, type);
      break;
    case 'Select': {
      assertKeys(node, ['type', 'props', 'press'], type);
      assertKeys(props, ['key', 'options', 'value', 'label', 'autoFocus'], `${type}.props`);
      nonEmptyString(props.key, `${type}.props.key`);
      if (!Array.isArray(props.options) || props.options.length < 1 || props.options.length > 64) {
        throw new TypeError('Select.props.options must contain 1 to 64 entries');
      }
      const seen = new Set<string>();
      for (const [index, optionValue] of props.options.entries()) {
        const option = requiredRecord(optionValue, `Select.props.options[${index}]`);
        assertKeys(option, ['value', 'label'], `Select.props.options[${index}]`);
        const optionKey = requiredString(option.value, `Select.props.options[${index}].value`);
        optionalString(option, 'label', 'Select option');
        if (seen.has(optionKey)) throw new TypeError('Select option values must be unique');
        seen.add(optionKey);
      }
      optionalString(props, 'value', type);
      optionalString(props, 'label', type);
      optionalTrue(props, 'autoFocus', type);
      parentPress(node.press, type);
      break;
    }
    case 'Link':
      assertKeys(node, ['type', 'props', 'children'], type);
      assertKeys(props, ['href', 'label'], `${type}.props`);
      const href = requiredString(props.href, `${type}.props.href`);
      if (!href.trim() || utf16Length(href) > 2_048) throw new TypeError('Link.props.href is invalid');
      if (props.label !== undefined) {
        const label = requiredString(props.label, `${type}.props.label`);
        if (!label.trim() || utf16Length(label) > 10_000) throw new TypeError('Link.props.label is invalid');
      }
      break;
    case 'Code':
      assertKeys(node, ['type', 'props'], type);
      assertKeys(props, ['source', 'language', 'path', 'startLine', 'format', 'wrap'], `${type}.props`);
      requiredString(props.source, `${type}.props.source`);
      optionalString(props, 'language', type);
      optionalString(props, 'path', type);
      if (props.startLine !== undefined && (typeof props.startLine !== 'number' || !Number.isSafeInteger(props.startLine)
        || props.startLine < 1 || props.startLine > 1_000_000_000)) throw new TypeError('Code.props.startLine is invalid');
      if (props.format !== undefined && props.format !== 'source' && props.format !== 'diff') throw new TypeError('Code.props.format is invalid');
      if (props.wrap !== undefined && props.wrap !== 'wrap' && props.wrap !== 'truncate-end') throw new TypeError('Code.props.wrap is invalid');
      break;
    case 'Markdown': {
      assertKeys(node, ['type', 'props', 'press'], type);
      assertKeys(props, ['key', 'text', 'dimColor', 'pressableLinks'], `${type}.props`);
      requiredString(props.text, `${type}.props.text`);
      if (props.key !== undefined) requiredString(props.key, `${type}.props.key`);
      if (props.key === '') throw new TypeError('Markdown.props.key must be non-empty');
      if (props.dimColor !== undefined && typeof props.dimColor !== 'boolean') throw new TypeError('Markdown.props.dimColor must be boolean');
      if (props.pressableLinks !== undefined) {
        if (!Array.isArray(props.pressableLinks) || props.pressableLinks.length > 256) throw new TypeError('Markdown.props.pressableLinks is invalid');
        for (const [index, linkValue] of props.pressableLinks.entries()) {
          const link = nonEmptyString(linkValue, `Markdown.props.pressableLinks[${index}]`);
          if (utf16Length(link) > 2_048) throw new TypeError('Markdown.props.pressableLinks is invalid');
        }
      }
      if (node.press !== undefined) {
        parentPress(node.press, type);
        if (typeof props.key !== 'string' || props.key.length === 0) throw new TypeError('Pressable Markdown requires props.key');
      } else if (Array.isArray(props.pressableLinks) && props.pressableLinks.length > 0) {
        throw new TypeError('Markdown pressableLinks require a press identity');
      }
      break;
    }
    case 'Svg': {
      assertKeys(node, ['type', 'props'], type);
      assertKeys(props, ['source', 'alt', 'width', 'height', 'isInteractive'], `${type}.props`);
      const source = requiredString(props.source, `${type}.props.source`);
      const alt = requiredString(props.alt, `${type}.props.alt`);
      if (utf16Length(source) > 131_072 || utf16Length(alt) > 10_000 || /[\u0000-\u001f\u007f-\u009f]/.test(alt)) {
        throw new TypeError('Svg.props source or alt exceeds the Native bounds');
      }
      for (const key of ['width', 'height']) {
        const dimension = props[key];
        if (dimension !== undefined && (typeof dimension !== 'number' || !Number.isFinite(dimension)
          || dimension <= 0 || dimension > 4_096)) throw new TypeError(`Svg.props.${key} is invalid`);
      }
      if (props.isInteractive !== undefined && typeof props.isInteractive !== 'boolean') throw new TypeError('Svg.props.isInteractive must be boolean');
      break;
    }
  }
}

function subtreeHasParentHover(value: unknown): boolean {
  if (!isRecord(value)) return false;
  return value.hover !== undefined || (Array.isArray(value.children) && value.children.some(subtreeHasParentHover));
}

/** Validates the 2.1.289 Parent UI tree contract before rendering it in React. */
export function parseModUiParentTree(value: unknown): UiJsonValue | null {
  if (value === null) return null;
  let serialized: string;
  try {
    serialized = JSON.stringify(value);
  } catch {
    throw new TypeError('Parent UI tree must be serializable JSON');
  }
  if (serialized === undefined || utf16Length(serialized) > MAX_PARENT_TREE_UTF16) {
    throw new TypeError('Parent UI tree exceeds the Native serialized-data limit');
  }
  validateParentNode(value, undefined, { nodes: 0 }, 0, false);
  return value as UiJsonValue;
}

export function collectModUiClientElements(tree: UiJsonValue | null): ModUiParentClientDescriptor[] {
  if (tree === null) return [];
  const parsed = parseModUiParentTree(tree);
  const clients: ModUiParentClientDescriptor[] = [];
  const seen = new Set<string>();
  const visit = (value: unknown): void => {
    if (!isRecord(value)) return;
    if (value.type === 'Client') {
      const client = validateClientElement(value);
      const identity = `${client.plugin}\u0000${client.key}`;
      if (seen.has(identity)) throw new TypeError('Native ui.render draws a Client key more than once for one plugin');
      seen.add(identity);
      clients.push(client);
      return;
    }
    if (Array.isArray(value.children)) for (const child of value.children) visit(child);
  };
  visit(parsed);
  return clients;
}

export function isClientControlAddressable(client: ModUiClientIdentity): boolean {
  return client.plugin.length <= 256 && client.key.length <= 256 && client.module.length <= 256;
}

export function buildNativeUiParentPressRequest(
  site: ModUiParentSiteIdentity,
  press: ParentPress,
  key: string,
  href?: string,
): Extract<NativeUiParentControlRequest, { subtype: 'ui_press' }> {
  return {
    subtype: 'ui_press', plugin: press.plugin, handle: press.handle, key, surface: site.surface,
    ...(href === undefined ? {} : { href }),
  };
}

export function buildNativeUiParentInputRequest(
  site: ModUiParentSiteIdentity,
  press: ParentPress,
  key: string,
  kind: 'change' | 'submit',
  value: string,
): Extract<NativeUiParentControlRequest, { subtype: 'ui_input' }> {
  return {
    subtype: 'ui_input', plugin: press.plugin, handle: press.handle, key, kind, value,
    component: site.component, instance_id: site.instanceId, surface: site.surface,
  };
}

export function buildNativeUiParentSelectRequest(
  site: ModUiParentSiteIdentity,
  press: ParentPress,
  key: string,
  value: string,
): Extract<NativeUiParentControlRequest, { subtype: 'ui_select' }> {
  return {
    subtype: 'ui_select', plugin: press.plugin, handle: press.handle, key, value,
    component: site.component, instance_id: site.instanceId, surface: site.surface,
  };
}

function buttonStyle(props: ParentRecord, theme: ReturnType<typeof useT>): CSSProperties {
  const plain = props.plain === true;
  const primary = props.variant === 'primary';
  return {
    alignSelf: 'flex-start',
    border: plain ? 0 : `1px solid ${theme.border}`,
    borderRadius: 7,
    background: plain ? 'transparent' : primary ? theme.accent : theme.surface,
    color: props.dimColor === true ? theme.text3 : primary ? '#fff' : theme.text,
    cursor: 'pointer',
    font: 'inherit',
    padding: plain ? '3px 6px' : '6px 10px',
    opacity: props.dimColor === true ? 0.72 : 1,
  };
}

function hoverStyles(type: string, props: ParentRecord, hover: ParentHover, theme: ReturnType<typeof useT>): CSSProperties {
  if (type === 'Box') return modUiBoxStyle({ ...props, ...hover } as Record<string, string | number | boolean>);
  if (type === 'Text') return modUiTextStyle({ ...props, ...hover } as Record<string, string | number | boolean>);
  const style = buttonStyle(props, theme);
  if (typeof hover.color === 'string') style.color = hover.color;
  if (typeof hover.backgroundColor === 'string') style.backgroundColor = hover.backgroundColor;
  if (hover.dimColor === true) style.opacity = 0.72;
  if (hover.bold === true) style.fontWeight = 600;
  if (hover.italic === true) style.fontStyle = 'italic';
  if (hover.underline === true || hover.strikethrough === true) {
    style.textDecoration = [hover.underline === true ? 'underline' : '', hover.strikethrough === true ? 'line-through' : ''].filter(Boolean).join(' ');
  }
  if (hover.inverse === true) {
    style.color = 'var(--mod-ui-inverse-fg, Canvas)';
    style.backgroundColor = 'var(--mod-ui-inverse-bg, CanvasText)';
  }
  return style;
}

function styleForNode(type: string, props: ParentRecord, hover: ParentHover | undefined, hovered: boolean, theme: ReturnType<typeof useT>): CSSProperties | undefined {
  if (type === 'Box') return modUiBoxStyle((hovered && hover ? { ...props, ...hover } : props) as Record<string, string | number | boolean>);
  if (type === 'Text') return modUiTextStyle((hovered && hover ? { ...props, ...hover } : props) as Record<string, string | number | boolean>);
  if (type === 'Button') return hovered && hover ? hoverStyles(type, props, hover, theme) : buttonStyle(props, theme);
  return undefined;
}

function hoverGroupKey(node: ParentNode, inheritedScope?: string, inheritedPlugin?: string): { key?: string; childScope?: string; childPlugin?: string } {
  const props = isRecord(node.props) ? node.props : {};
  const hover = isRecord(node.hover) ? node.hover : undefined;
  const group = isRecord(node.group) && typeof node.group.plugin === 'string' ? node.group.plugin : undefined;
  const press = isRecord(node.press) && typeof node.press.plugin === 'string' ? node.press.plugin : undefined;
  const keyedBox = node.type === 'Box' && typeof props.key === 'string' ? props.key : undefined;
  const scope = typeof hover?.scope === 'string' ? hover.scope : keyedBox ?? inheritedScope;
  const plugin = group ?? press ?? inheritedPlugin;
  const key = hover === undefined || scope === undefined ? undefined : JSON.stringify([plugin ?? '', scope]);
  return { key, childScope: keyedBox ?? inheritedScope, childPlugin: group ?? inheritedPlugin };
}

function parentChildIdentity(value: unknown, index: number): string {
  if (!isRecord(value) || typeof value.type !== 'string') return `index:${index}`;
  const props = isRecord(value.props) ? value.props : {};
  if (value.type === 'Client' && isRecord(value.client)
    && typeof value.client.plugin === 'string' && typeof props.key === 'string' && typeof props.module === 'string') {
    return `Client:${JSON.stringify([value.client.plugin, props.key, props.module])}`;
  }
  if (['Box', 'Text', 'Button', 'Input', 'Select', 'Markdown'].includes(value.type)
    && typeof props.key === 'string') {
    return `${value.type}:${JSON.stringify(props.key)}`;
  }
  return `index:${index}`;
}

function validateAndStyleDeclaration(text: unknown): CSSProperties {
  if (typeof text !== 'string') return {};
  const style: Record<string, string | number> = {};
  let start = 0;
  let quote: string | null = null;
  let depth = 0;
  const declarations: string[] = [];
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if (quote !== null) {
      if (character === quote && text[index - 1] !== '\\') quote = null;
      continue;
    }
    if (character === '"' || character === "'") quote = character;
    else if (character === '(') depth += 1;
    else if (character === ')' && depth > 0) depth -= 1;
    else if (character === ';' && depth === 0) {
      declarations.push(text.slice(start, index));
      start = index + 1;
    }
  }
  declarations.push(text.slice(start));
  for (const declaration of declarations) {
    const separator = declaration.indexOf(':');
    if (separator < 1) continue;
    const property = declaration.slice(0, separator).trim();
    const value = declaration.slice(separator + 1).trim();
    if (!value || !/^(?:--[\w-]+|-?[A-Za-z][A-Za-z0-9-]*)$/.test(property)) continue;
    if (/(?:url\s*\(|expression\s*\(|javascript\s*:|@import)/i.test(value)) continue;
    const reactProperty = property.startsWith('--') ? property : property.replace(/-([a-z])/g, (_match, letter: string) => letter.toUpperCase());
    style[reactProperty] = value;
  }
  return style as CSSProperties;
}

function legacyHtmlProps(props: ParentRecord): React.HTMLAttributes<HTMLElement> {
  const output: Record<string, unknown> = {};
  for (const [rawName, value] of Object.entries(props)) {
    if (rawName === 'style') {
      output.style = validateAndStyleDeclaration(value);
      continue;
    }
    if (/^on/i.test(rawName) || ['children', 'dangerouslySetInnerHTML', 'key', 'ref', 'innerHTML'].includes(rawName)) continue;
    if (!/^[A-Za-z][A-Za-z0-9:._-]*$/.test(rawName)) continue;
    const normalized = rawName === 'class' ? 'className'
      : rawName === 'for' ? 'htmlFor'
        : rawName === 'tabindex' ? 'tabIndex'
          : rawName;
    output[normalized] = value;
  }
  return output as React.HTMLAttributes<HTMLElement>;
}

function safeNativeLink(value: string): string | undefined {
  if (value.length > 2_048 || /[^\x20-\x7e]/.test(value) || value.includes('@')) return undefined;
  try {
    const url = new URL(value);
    if (url.username || url.password) return undefined;
    if (url.protocol === 'https:' || url.protocol === 'http:' && url.hostname === 'localhost') {
      return url.href.length <= 2_048 ? url.href : undefined;
    }
  } catch {
    // The engine validates remote links; malformed values remain inert at this boundary.
  }
  return undefined;
}

function svgDataUrl(source: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(source)}`;
}

function svgSandboxDocument(source: string): string {
  return `<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data: blob:; style-src 'unsafe-inline'; script-src 'unsafe-inline'"><body style="margin:0">${source}</body>`;
}

function sendControl(onParentControl: ModUiParentTreeProps['onParentControl'], request: NativeUiParentControlRequest): void {
  try {
    void Promise.resolve(onParentControl(request)).catch(() => undefined);
  } catch {
    // A stale or unavailable parent control must not turn into a Client fault.
  }
}

function ParentNodeView({
  value,
  path,
  site,
  requestProps,
  responseProps,
  fallback,
  engineFallback,
  renderClient,
  onParentControl,
  inheritedScope,
  inheritedPlugin,
}: {
  value: ParentNode | string;
  path: string;
  site: ModUiParentSiteIdentity;
  requestProps: Record<string, UiJsonValue>;
  responseProps: Record<string, UiJsonValue>;
  fallback?: ReactNode;
  engineFallback?: ModUiParentEngineFallback;
  renderClient: ModUiParentTreeProps['renderClient'];
  onParentControl: ModUiParentTreeProps['onParentControl'];
  inheritedScope?: string;
  inheritedPlugin?: string;
}): React.ReactNode {
  const hoverContext = useContext(ParentHoverContext);
  const theme = useT();
  if (typeof value === 'string') return value;
  const node = value;
  const props = isRecord(node.props) ? node.props : {};
  const hover = isRecord(node.hover) ? node.hover : undefined;
  const children = Array.isArray(node.children) ? node.children : [];
  const { key: hoverKey, childScope, childPlugin } = hoverGroupKey(node, inheritedScope, inheritedPlugin);
  const hovered = hoverKey !== undefined && hoverContext.activeGroup === hoverKey;
  const style = styleForNode(node.type, props, hover, hovered, theme);
  const hoverAttrs = hoverKey === undefined ? {} : {
    'data-mod-ui-hover-group': hoverKey,
    'data-mod-ui-hover-scope': hover?.scope ?? (node.type === 'Box' ? props.key : inheritedScope),
    ...(isRecord(node.group) && typeof node.group.plugin === 'string' ? { 'data-mod-ui-hover-plugin': node.group.plugin } : {}),
  };
  const onPointerEnter = hoverKey === undefined ? undefined : () => hoverContext.setActiveGroup(hoverKey);
  const onPointerLeave = hoverKey === undefined ? undefined : (event: React.PointerEvent<HTMLElement>) => {
    const related = event.relatedTarget;
    if (related instanceof Element && related.closest('[data-mod-ui-hover-group]')?.getAttribute('data-mod-ui-hover-group') === hoverKey) return;
    if (hoverContext.activeGroup === hoverKey) hoverContext.setActiveGroup(null);
  };
  const childNodes = () => {
    const identityCounts = new Map<string, number>();
    return children.map((child, index) => {
      const identity = parentChildIdentity(child, index);
      const occurrence = identityCounts.get(identity) ?? 0;
      identityCounts.set(identity, occurrence + 1);
      const childPath = `${path}:${identity}${occurrence === 0 ? '' : `:${occurrence}`}`;
      return <ParentNodeView
        key={childPath}
        value={child as ParentNode | string}
        path={childPath}
        site={site}
        requestProps={requestProps}
        responseProps={responseProps}
        fallback={fallback}
        engineFallback={engineFallback}
        renderClient={renderClient}
        onParentControl={onParentControl}
        inheritedScope={childScope}
        inheritedPlugin={childPlugin}
      />;
    });
  };

  switch (node.type) {
    case 'Box': return <div key={path} className="mod-ui-parent-box" style={style} {...hoverAttrs} onPointerEnter={onPointerEnter} onPointerLeave={onPointerLeave}>{childNodes()}</div>;
    case 'Text': return <span key={path} className="mod-ui-parent-text" style={style} {...hoverAttrs} onPointerEnter={onPointerEnter} onPointerLeave={onPointerLeave}>{childNodes()}</span>;
    case 'div':
    case 'span':
    case 'b': {
      const Tag = node.type;
      const legacyProps = legacyHtmlProps(props);
      const originalClass = typeof legacyProps.className === 'string' ? legacyProps.className : '';
      return <Tag key={path} {...legacyProps} className={`mod-ui-parent-html mod-ui-parent-html-${Tag}${originalClass ? ` ${originalClass}` : ''}`}>{childNodes()}</Tag>;
    }
    case 'Button': {
      const stamp = parentPress(node.press, 'Button');
      const key = String(props.key);
      const request = buildNativeUiParentPressRequest(site, stamp, key);
      return <button
        key={path}
        type="button"
        className="mod-ui-parent-button"
        style={style}
        aria-keyshortcuts={typeof props.hotkey === 'string' ? props.hotkey : undefined}
        aria-label={props.role === 'dismiss' ? String(props.label) : undefined}
        autoFocus={props.autoFocus === true}
        data-action={props.action}
        data-role={props.role}
        {...hoverAttrs}
        onPointerEnter={onPointerEnter}
        onPointerLeave={onPointerLeave}
        onFocus={onPointerEnter}
        onBlur={() => { if (hoverContext.activeGroup === hoverKey) hoverContext.setActiveGroup(null); }}
        onClick={() => sendControl(onParentControl, request)}
      >{String(props.label)}</button>;
    }
    case 'Input':
      return <ParentInput key={path} node={node} site={site} onParentControl={onParentControl} />;
    case 'Select':
      return <ParentSelect key={path} node={node} site={site} onParentControl={onParentControl} />;
    case 'Link': {
      const href = safeNativeLink(String(props.href));
      const label = typeof props.label === 'string' ? props.label : String(props.href);
      const content = children.length > 0 ? childNodes() : label;
      return href
        ? <a key={path} className="mod-ui-parent-link" href={href} target="_blank" rel="noreferrer">{content}</a>
        : <span key={path} className="mod-ui-parent-link-disabled">{content}</span>;
    }
    case 'Code': {
      return <ParentCode key={path} props={props} />;
    }
    case 'Markdown': {
      const press = node.press === undefined ? undefined : parentPress(node.press, 'Markdown');
      const key = typeof props.key === 'string' ? props.key : '';
      const onClickCapture = press ? (event: React.MouseEvent<HTMLDivElement>) => {
        const target = event.target;
        if (!(target instanceof Element)) return;
        const anchor = target.closest('a[href]');
        const href = anchor?.getAttribute('href');
        if (href === null || href === undefined) return;
        event.preventDefault();
        sendControl(onParentControl, buildNativeUiParentPressRequest(site, press, key, href));
      } : undefined;
      return <div key={path} className="mod-ui-parent-markdown" data-pressable-links={Array.isArray(props.pressableLinks) ? props.pressableLinks.join(' ') : undefined} style={props.dimColor === true ? { color: theme.text3 } : undefined} onClickCapture={onClickCapture}>
        <MarkdownContent text={String(props.text)} />
      </div>;
    }
    case 'Client': {
      const descriptor = validateClientElement(node);
      return renderClient(descriptor, path);
    }
    case 'Svg': {
      const source = String(props.source);
      const alt = String(props.alt);
      const width = typeof props.width === 'number' ? `${props.width}px` : undefined;
      const height = typeof props.height === 'number' ? `${props.height}px` : undefined;
      const style: CSSProperties = { display: 'block', width: width ?? 'auto', height: height ?? 'auto', maxWidth: '100%', objectFit: 'contain' };
      if (props.isInteractive === true) {
        return <iframe key={path} className="mod-ui-parent-svg" title={alt} sandbox="allow-scripts" srcDoc={svgSandboxDocument(source)} style={style} />;
      }
      return <img key={path} className="mod-ui-parent-svg" src={svgDataUrl(source)} alt={alt} style={style} />;
    }
    case 'engine': {
      const ref = Number(node.ref);
      const context = { ref, requestProps, responseProps };
      if (typeof engineFallback === 'function') {
        try {
          return engineFallback(context);
        } catch {
          return fallback ?? null;
        }
      }
      if (engineFallback !== undefined) return engineFallback;
      return ref === 0 ? fallback ?? null : null;
    }
    default: return null;
  }
}

function ParentInput({ node, site, onParentControl }: {
  node: ParentNode;
  site: ModUiParentSiteIdentity;
  onParentControl: ModUiParentTreeProps['onParentControl'];
}): React.ReactElement {
  const props = isRecord(node.props) ? node.props : {};
  const stamp = parentPress(node.press, 'Input');
  const key = String(props.key);
  const value = typeof props.value === 'string' ? props.value : '';
  const [draft, setDraft] = useState(value);
  const id = useId();
  useEffect(() => setDraft(value), [key, value]);
  const submit = useCallback((event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const input = form.querySelector('input');
    sendControl(onParentControl, buildNativeUiParentInputRequest(site, stamp, key, 'submit', input?.value ?? draft));
  }, [draft, key, onParentControl, site, stamp]);
  return <form className="mod-ui-parent-input" onSubmit={submit}>
    <label htmlFor={`mod-ui-parent-input-${id}`} className="mod-ui-parent-input-label">
      {typeof props.label === 'string' && props.label.length > 0 ? <span>{props.label}</span> : null}
        <input
        id={`mod-ui-parent-input-${id}`}
        type="text"
        value={draft}
        placeholder={typeof props.placeholder === 'string' ? props.placeholder : undefined}
        autoFocus={props.autoFocus === true}
        onChange={(event) => {
          const next = event.currentTarget.value;
          setDraft(next);
          sendControl(onParentControl, buildNativeUiParentInputRequest(site, stamp, key, 'change', next));
        }}
      />
    </label>
    {typeof props.submitLabel === 'string' && props.submitLabel.length > 0
      ? <button type="submit" className="mod-ui-parent-input-submit">{props.submitLabel}</button>
      : null}
  </form>;
}

function ParentCode({ props }: { props: ParentRecord }): React.ReactElement {
  const theme = useT();
  const source = typeof props.source === 'string' ? props.source : '';
  const language = props.format === 'diff' ? 'diff' : typeof props.language === 'string' ? props.language : undefined;
  const startLine = typeof props.startLine === 'number' ? props.startLine : undefined;
  const wrap = props.wrap === 'truncate-end' ? 'truncate-end' : 'wrap';
  const lines = source.split(/\r?\n/);
  return <figure
    className="mod-ui-parent-code"
    data-path={props.path}
    data-start-line={startLine}
    data-format={props.format}
    data-wrap={wrap}
    data-language={language}
    style={{ minWidth: 0, margin: 0 }}
  >
    {(typeof props.path === 'string' || startLine !== undefined) && <figcaption className="mod-ui-parent-code-caption">
      {typeof props.path === 'string' ? props.path : ''}{typeof props.path === 'string' && startLine !== undefined ? ' · ' : ''}{startLine !== undefined ? `Line ${startLine}` : ''}
    </figcaption>}
    <pre className={`mod-ui-parent-code-pre mod-ui-parent-code-${wrap}`} style={{ margin: 0, minWidth: 0, overflowX: wrap === 'truncate-end' ? 'hidden' : 'auto', whiteSpace: 'normal' }}>
      <code data-language={language} style={{ display: 'block', minWidth: 0, color: theme.text }}>
        {lines.map((line, index) => {
          const highlighted = highlightCodeForDisplay(line, language, true);
          return <span
            key={index}
            className="mod-ui-parent-code-line"
            style={{
              display: 'grid',
              gridTemplateColumns: startLine === undefined ? 'minmax(0, 1fr)' : 'auto minmax(0, 1fr)',
              gap: startLine === undefined ? 0 : 12,
              minWidth: 0,
              whiteSpace: wrap === 'truncate-end' ? 'nowrap' : 'pre-wrap',
              overflow: wrap === 'truncate-end' ? 'hidden' : 'visible',
              textOverflow: wrap === 'truncate-end' ? 'ellipsis' : undefined,
            }}
          >
            {startLine !== undefined && <span className="mod-ui-parent-code-line-number" aria-hidden="true" style={{ color: theme.text3, userSelect: 'none', textAlign: 'right' }}>{startLine + index}</span>}
            {highlighted.html === undefined
              ? <span>{line || '\u00a0'}</span>
              : <span dangerouslySetInnerHTML={{ __html: highlighted.html || '\u00a0' }} />}
          </span>;
        })}
      </code>
    </pre>
  </figure>;
}

function ParentSelect({ node, site, onParentControl }: {
  node: ParentNode;
  site: ModUiParentSiteIdentity;
  onParentControl: ModUiParentTreeProps['onParentControl'];
}): React.ReactElement {
  const theme = useT();
  const props = isRecord(node.props) ? node.props : {};
  const stamp = parentPress(node.press, 'Select');
  const key = String(props.key);
  const options = Array.isArray(props.options) ? props.options.filter(isRecord) : [];
  const firstOption = options[0];
  const initialValue = typeof props.value === 'string' ? props.value : typeof firstOption?.value === 'string' ? firstOption.value : '';
  const [selected, setSelected] = useState(initialValue);
  const id = useId();
  useEffect(() => setSelected(initialValue), [key, initialValue]);
  return <label htmlFor={`mod-ui-parent-select-${id}`} className="mod-ui-parent-select-label">
    {typeof props.label === 'string' && props.label.length > 0 ? <span>{props.label}</span> : null}
    <select
      id={`mod-ui-parent-select-${id}`}
      value={selected}
      autoFocus={props.autoFocus === true}
      onChange={(event) => {
        const value = event.currentTarget.value;
        setSelected(value);
        sendControl(onParentControl, buildNativeUiParentSelectRequest(site, stamp, key, value));
      }}
      style={{ minWidth: 0, border: `1px solid ${theme.border}`, borderRadius: 7, padding: '7px 9px', background: theme.windowBg, color: theme.text, font: 'inherit' }}
    >
      {options.map((option, index) => (
        <option key={`${String(option.value)}:${index}`} value={typeof option.value === 'string' ? option.value : ''}>
          {typeof option.label === 'string' ? option.label : String(option.value ?? '')}
        </option>
      ))}
    </select>
  </label>;
}

export const ModUiParentTree = memo(function ModUiParentTree({
  tree,
  site,
  requestProps,
  responseProps,
  fallback,
  engineFallback,
  renderClient,
  onParentControl,
}: ModUiParentTreeProps) {
  const [activeGroup, setActiveGroup] = useState<string | null>(null);
  const hoverContext = useMemo(() => ({ activeGroup, setActiveGroup }), [activeGroup]);
  const parsed = useMemo(() => {
    try {
      return parseModUiParentTree(tree);
    } catch {
      return null;
    }
  }, [tree]);
  if (parsed === null) return <>{fallback}</>;
  return <ParentHoverContext.Provider value={hoverContext}>
    <ParentNodeView
      value={parsed as ParentNode | string}
      path="root"
      site={site}
      requestProps={requestProps}
      responseProps={responseProps}
      fallback={fallback}
      engineFallback={engineFallback}
      renderClient={renderClient}
      onParentControl={onParentControl}
    />
  </ParentHoverContext.Provider>;
});
