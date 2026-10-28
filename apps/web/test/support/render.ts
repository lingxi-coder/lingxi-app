import { createRequire } from 'node:module';
import type { ReactElement } from 'react';

const { create, act } = createRequire(import.meta.url)('react-test-renderer');

export { act };

type Json = null | string | JsonElement | Json[];
interface JsonElement {
  type: string;
  props: Record<string, unknown>;
  children: Json[] | null;
}

/**
 * What a rendering shows, as text: the tags, the attributes that are not
 * handlers, and the text. Two renderings, or a rendering and a literal, can
 * be compared without a browser.
 */
export function markup(node: Json): string {
  if (node === null) return '';
  if (typeof node === 'string') return node;
  if (Array.isArray(node)) return node.map(markup).join('');
  const attributes = Object.entries(node.props)
    .filter(([, value]) => typeof value !== 'function' && value !== undefined && value !== null && value !== false)
    .map(([name, value]) => (value === true ? ` ${name}` : ` ${name}="${String(value)}"`))
    .join('');
  return `<${node.type}${attributes}>${(node.children ?? []).map(markup).join('')}</${node.type}>`;
}

/** The text inside an element of the test renderer's tree. */
export function textOf(node: any): string {
  return typeof node === 'string' ? node : node.children.map(textOf).join('');
}

const hasClass = (node: any, name: string): boolean => String(node.props.className ?? '').split(' ').includes(name);

/** A rendering that stays open while a test drives and reads it. */
export class Mounted {
  private constructor(
    private readonly renderer: any,
    private readonly wait: () => Promise<void>,
  ) {}

  /** Render `element`; `wait` lets what is on its way arrive (the fake clock, the event loop) and runs inside `act`. */
  static async create(element: ReactElement, wait: () => Promise<void> = async () => undefined): Promise<Mounted> {
    let renderer: any;
    await act(async () => {
      renderer = create(element);
      await wait();
    });
    const mounted = new Mounted(renderer, wait);
    await mounted.settle();
    return mounted;
  }

  get markup(): string {
    return markup(this.renderer.toJSON());
  }

  /** Nothing is rendered. */
  get empty(): boolean {
    return this.renderer.toJSON() === null;
  }

  /** The elements a selector names, in tree order: `tag`, `.class`, or a descendant of the first by the second (`.class tag`). */
  private select(selector: string): any[] {
    let scope: any[] = [this.renderer.root];
    for (const part of selector.split(' ')) {
      const found = new Set<any>();
      for (const root of scope) {
        const matches = root.findAll((node: any) => node !== root && typeof node.type === 'string' && (part.startsWith('.') ? hasClass(node, part.slice(1)) : node.type === part));
        for (const node of matches) found.add(node);
      }
      scope = [...found];
    }
    return scope;
  }

  /** The text of every element a selector names, in tree order. */
  texts(selector: string): string[] {
    return this.select(selector).map(textOf);
  }

  /** The `aria-label` of the first element a selector names. */
  label(selector: string): string | undefined {
    return this.select(selector)[0]?.props['aria-label'];
  }

  /** The one button whose text is `text`. */
  button(text: string): any {
    const found = this.select('button').filter((node) => textOf(node) === text);
    if (found.length !== 1) throw new Error(`expected one button "${text}", found ${found.length} of: ${this.texts('button').join(' | ')}`);
    return found[0];
  }

  has(text: string): boolean {
    return this.select('button').some((node) => textOf(node) === text);
  }

  /** The one row (a button with a `<strong>` title) titled `title`. */
  private row(title: string): any {
    const found = this.select('button').filter((node) => node.findAll((inner: any) => inner.type === 'strong' && textOf(inner) === title).length > 0);
    if (found.length !== 1) throw new Error(`expected one row "${title}", found ${found.length} of: ${this.texts('strong').join(' | ')}`);
    return found[0];
  }

  async click(text: string): Promise<void> {
    await this.press(this.button(text));
  }

  async clickRow(title: string): Promise<void> {
    await this.press(this.row(title));
  }

  private async press(button: any): Promise<void> {
    if (button.props.disabled === true) throw new Error('a disabled button cannot be pressed');
    await act(async () => {
      button.props.onClick({ preventDefault() {} });
      await this.wait();
    });
    await this.settle();
  }

  /** Type into the one text input. */
  async type(text: string): Promise<void> {
    const inputs = this.select('input');
    if (inputs.length !== 1) throw new Error(`expected one input, found ${inputs.length}`);
    await act(async () => {
      inputs[0].props.onChange({ target: { value: text } });
      await this.wait();
    });
    await this.settle();
  }

  /** Whether the button is disabled, as the browser would not let it be pressed. */
  disabled(text: string): boolean {
    return this.button(text).props.disabled === true;
  }

  /**
   * Let what is on its way arrive, then render what it changed. Every action
   * settles once more when it is done: what the effects of a render ask for
   * goes out only after that render.
   */
  async settle(): Promise<void> {
    await act(async () => {
      await this.wait();
    });
  }

  /** Run something (a message from the computer, say), then render what it changed. */
  async during(work: () => Promise<void>): Promise<void> {
    await act(async () => {
      await work();
      await this.wait();
    });
    await this.settle();
  }

  async update(element: ReactElement): Promise<void> {
    await act(async () => {
      this.renderer.update(element);
      await this.wait();
    });
    await this.settle();
  }

  async unmount(): Promise<void> {
    await act(async () => this.renderer.unmount());
  }
}
