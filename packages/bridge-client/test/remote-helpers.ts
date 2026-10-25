/**
 * Shared helpers of the remote-control tests (not a test file itself).
 */

import { readFileSync } from 'node:fs';

export const hex = (bytes: Uint8Array): string => Buffer.from(bytes).toString('hex');

export const unhex = (text: string): Uint8Array => new Uint8Array(Buffer.from(text, 'hex'));

/** Load `test/fixtures/remote/<name>.json`. */
export function fixture<T = any>(name: string): T { // eslint-disable-line @typescript-eslint/no-explicit-any
  return JSON.parse(readFileSync(new URL(`./fixtures/remote/${name}.json`, import.meta.url), 'utf8')) as T;
}

/** Give promise continuations and WebCrypto jobs a few event-loop turns. */
export async function settle(turns = 5): Promise<void> {
  for (let i = 0; i < turns; i++) await new Promise<void>((resolve) => setImmediate(resolve));
}

/** Wait, in real time, until `condition()` holds. */
export async function until(condition: () => boolean, label = 'condition', timeoutMs = 5000): Promise<void> {
  const start = Date.now();
  while (!condition()) {
    if (Date.now() - start > timeoutMs) throw new Error(`timed out waiting for ${label}`);
    await new Promise<void>((resolve) => setImmediate(resolve));
  }
}

// ── Fake network for the RTDB client ─────────────────────────────────────────

/** One request as the fake `fetch` saw it. */
export interface FakeCall {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: string | undefined;
  signal: AbortSignal | undefined;
}

/** A JSON response. */
export function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
}

/** One server-sent event in wire format. */
export function sse(event: string, data: unknown): string {
  return `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
}

/**
 * A scripted `fetch`: every request records itself and takes the next queued
 * answer; with none queued it fails like a dead network.
 */
export class FakeFetch {
  readonly calls: FakeCall[] = [];
  private readonly script: Array<(call: FakeCall) => Response | Promise<Response>> = [];

  /** Queue the answer to the next unanswered request. */
  next(responder: (call: FakeCall) => Response | Promise<Response>): this {
    this.script.push(responder);
    return this;
  }

  /** Queue an event stream you drive by hand. */
  nextStream(options: { wireAbort?: boolean } = {}): FakeStream {
    const stream = new FakeStream(options.wireAbort ?? true);
    this.script.push((call) => stream.attach(call.signal));
    return stream;
  }

  readonly fetch = (input: string, init: RequestInit = {}): Promise<Response> => {
    const headers: Record<string, string> = {};
    new Headers(init.headers).forEach((value, key) => {
      headers[key] = value;
    });
    const call: FakeCall = {
      url: input,
      method: init.method ?? 'GET',
      headers,
      body: typeof init.body === 'string' ? init.body : undefined,
      signal: init.signal ?? undefined,
    };
    this.calls.push(call);
    const responder = this.script.shift();
    if (responder === undefined) return Promise.reject(new Error(`connection refused: ${call.method} ${call.url}`));
    try {
      return Promise.resolve(responder(call));
    } catch (error) {
      return Promise.reject(error);
    }
  };
}

/** A hand-driven `text/event-stream` body. */
export class FakeStream {
  readonly response: Response;
  private controller!: ReadableStreamDefaultController<Uint8Array>;
  private finished = false;
  /** Set once the client aborted the request. */
  aborted = false;

  constructor(private readonly wireAbort = true) {
    const body = new ReadableStream<Uint8Array>({
      start: (controller) => {
        this.controller = controller;
      },
    });
    this.response = new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
  }

  /** Connect the request's abort signal, as a real `fetch` does with the body. */
  attach(signal: AbortSignal | undefined): Response {
    const onAbort = (): void => {
      this.aborted = true;
      if (this.wireAbort) this.fail(new Error('aborted'));
    };
    if (signal?.aborted) onAbort();
    else signal?.addEventListener('abort', onAbort, { once: true });
    return this.response;
  }

  write(text: string): void {
    this.push(new TextEncoder().encode(text));
  }

  push(bytes: Uint8Array): void {
    if (this.finished) return;
    try {
      this.controller.enqueue(bytes);
    } catch {
      this.finished = true; // the reader cancelled the stream
    }
  }

  end(): void {
    if (this.finished) return;
    this.finished = true;
    try {
      this.controller.close();
    } catch {
      // already cancelled
    }
  }

  fail(error: Error): void {
    if (this.finished) return;
    this.finished = true;
    try {
      this.controller.error(error);
    } catch {
      // already cancelled
    }
  }
}
