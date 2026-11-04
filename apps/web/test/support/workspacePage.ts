import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test, { type TestContext } from 'node:test';
import { createElement, useSyncExternalStore, type ReactElement } from 'react';
import type { TeamsSnapshot, TeamsWorkspace } from '../../../../packages/teams/workspace';

// Tests that drive the Teams components' event handlers and effects go through react-test-renderer, which is a
// devDependency of this package. Where it is not installed they are skipped rather than failing to load.
const require = createRequire(import.meta.url);
let renderer: { create(element: ReactElement, options?: unknown): any; act(work: () => Promise<void> | void): Promise<void> } | undefined;
try { renderer = require('react-test-renderer'); } catch { renderer = undefined; }
export const scenario = (name: string, run: (context: TestContext) => Promise<void>) => test(name, renderer ? {} : { skip: 'react-test-renderer is not installed' }, run);
/** The test renderer itself, for a test that mounts something other than a view of one workspace. */
export const reactTestRenderer = () => renderer!;

// ── A just-enough browser: refs, the document, and HTMLElement ──

export function hostNode() {
  const node: Record<string, unknown> = {
    open: false, style: {}, scrollTop: 0, scrollHeight: 0, clientHeight: 0, offsetTop: 0, value: '', selectionStart: 0, selectionEnd: 0,
    focus() {}, blur() {}, select() {}, setSelectionRange() {}, setAttribute() {}, removeAttribute() {}, addEventListener() {}, removeEventListener() {},
    querySelector: () => null, querySelectorAll: () => [], closest: () => null, contains: () => false,
    getBoundingClientRect: () => ({ top: 0, bottom: 0, left: 0, right: 0, width: 0, height: 0 }),
    showModal() { node.open = true; }, close() { node.open = false; },
  };
  return node;
}
export function withDom(context: TestContext): void {
  const names = ['document', 'window', 'HTMLElement'] as const;
  const saved = names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  class FakeElement {}
  Object.defineProperty(globalThis, 'HTMLElement', { configurable: true, value: FakeElement });
  Object.defineProperty(globalThis, 'document', { configurable: true, value: { activeElement: null, addEventListener() {}, removeEventListener() {}, contains: () => true, getElementById: () => null } });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { innerHeight: 800, innerWidth: 1200, addEventListener() {}, removeEventListener() {} } });
  context.after(() => { for (const [name, descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } });
}

// ── Rendering and driving ──

export const settle = async (turns = 10) => { for (let i = 0; i < turns; i++) await new Promise(resolve => setTimeout(resolve, 0)); };
export function Host({ workspace, render }: { workspace: TeamsWorkspace; render(snapshot: TeamsSnapshot): ReactElement }): ReactElement {
  return render(useSyncExternalStore(workspace.subscribe, workspace.getSnapshot));
}
export const classes = (node: any) => String(node.props.className ?? '').split(' ');
export const textOf = (node: any): string => typeof node === 'string' ? node : (node.children ?? []).map(textOf).join('');

export async function mount(workspace: TeamsWorkspace, render: (snapshot: TeamsSnapshot) => ReactElement) {
  const { create, act } = renderer!;
  let tree: any;
  await act(async () => { tree = create(createElement(Host, { workspace, render }), { createNodeMock: hostNode }); await settle(); });
  const find = (match: (node: any) => boolean): any[] => tree.root.findAll((node: any) => typeof node.type === 'string' && match(node));
  const one = (match: (node: any) => boolean, label: string): any => {
    const found = find(match);
    if (found.length !== 1) throw new Error(`expected one ${label}, found ${found.length}`);
    return found[0];
  };
  const page = {
    find,
    byClass: (name: string) => find(node => classes(node).includes(name)),
    oneByClass: (name: string) => one(node => classes(node).includes(name), `.${name}`),
    byLabel: (label: string) => find(node => node.props['aria-label'] === label),
    oneByLabel: (label: string) => one(node => node.props['aria-label'] === label, `[aria-label="${label}"]`),
    button: (text: string) => one(node => node.type === 'button' && textOf(node) === text, `button "${text}"`),
    has: (text: string) => find(node => node.type === 'button' && textOf(node) === text).length > 0,
    text: () => textOf(tree.toJSON() === null ? '' : { children: [].concat(tree.toJSON()).map(function walk(n: any): any { return typeof n === 'string' ? n : { children: (n.children ?? []).map(walk) }; }) }),
    async during(work: () => void | Promise<void>) { await act(async () => { await work(); await settle(); }); await act(async () => { await settle(); }); },
    async click(node: any) { assert.notEqual(node.props.disabled, true, 'a disabled control cannot be pressed'); await page.during(() => node.props.onClick({ preventDefault() {}, stopPropagation() {} })); },
    async check(node: any, checked = true) { await page.during(() => node.props.onChange({ target: { checked } })); },
    async submit(form: any = one(node => node.type === 'form', 'form')) { await page.during(() => form.props.onSubmit({ preventDefault() {} })); },
    async type(node: any, value: string, caret = value.length) { await page.during(() => node.props.onChange({ target: { value, selectionStart: caret }, currentTarget: { value, selectionStart: caret } })); },
    async key(node: any, key: string, more: Record<string, unknown> = {}) {
      let prevented = false;
      await page.during(() => node.props.onKeyDown({ key, shiftKey: false, keyCode: key === 'Enter' ? 13 : 0, nativeEvent: { isComposing: false }, preventDefault() { prevented = true; }, stopPropagation() {}, defaultPrevented: false, target: node, currentTarget: node, ...more }));
      return prevented;
    },
    async unmount() { await act(async () => tree.unmount()); },
  };
  return page;
}
