import type { ChatEvent, ChatTurn, ProviderTestResult, VaultProvider, VaultUpsert } from '../../../../../packages/llm-vault/contracts';
import type { ResourceRef } from '../../../../../packages/teams/dataset-resource';
import { createSseParser } from './sse';

/**
 * The Web's client of llmApi. Requests go to the function URL directly, not through Hosting: a Hosting rewrite ends a
 * response at 60 seconds and may buffer a stream. `VITE_LLM_API_URL` points development at a local llmApi.
 */
export const LLM_API_URL: string = (import.meta as { env?: Record<string, string | undefined> }).env?.VITE_LLM_API_URL
  || 'https://us-central1-lingxi-5e4b2.cloudfunctions.net/llmApi';

export class LlmError extends Error {
  constructor(readonly code: string) { super(code); }
}

export type DatasetAttachment = Extract<ResourceRef, { kind: 'dataset' }>;

export type DatasetTurnRequest =
  | { action: 'start'; id: string; chat: ChatTurn; attachment?: DatasetAttachment }
  | { action: 'get' | 'advance' | 'cancel'; id: string }
  | { action: 'approve' | 'deny'; id: string; approval: string };

export interface DatasetToolProposal {
  id: string;
  name: 'dataset_create' | 'dataset_put' | 'dataset_latest';
  arguments: Record<string, unknown>;
}

export type DatasetTurn = { id: string; answer: string } & (
  | { state: 'approval'; approval: string; calls: DatasetToolProposal[] }
  | { state: 'generating' | 'executing' | 'ready' | 'done' | 'denied' | 'cancelled' }
  | { state: 'failed'; code: string }
);

export interface LlmClient {
  datasetTurn?(request: DatasetTurnRequest, signal?: AbortSignal): Promise<DatasetTurn>;
  listProviders(): Promise<VaultProvider[]>;
  upsertProvider(input: VaultUpsert): Promise<VaultProvider>;
  deleteProvider(id: string, expectedRevision?: number): Promise<void>;
  testProvider(id: string, model?: string): Promise<ProviderTestResult>;
  /** Streams one turn; `onEvent` sees every event, and the promise settles with the last one (`done` or `error`). */
  streamChat(turn: ChatTurn, onEvent: (event: ChatEvent) => void, signal?: AbortSignal): Promise<ChatEvent>;
}

/** `token` answers the signed-in user's ID token, or throws when the session is no longer that user's. */
export function createLlmClient(token: () => Promise<string>, base = LLM_API_URL, fetcher: typeof fetch = (...args) => fetch(...args)): LlmClient {
  const call = async (path: string, init: RequestInit = {}) => {
    const response = await fetcher(`${base}${path}`, { ...init, credentials: 'omit', referrerPolicy: 'no-referrer',
      headers: { ...(init.body ? { 'Content-Type': 'application/json' } : {}), Authorization: `Bearer ${await token()}` } })
      .catch(error => { if ((error as Error)?.name === 'AbortError') throw error; throw new LlmError('NETWORK'); });
    if (!response.ok) {
      const body = await response.json().catch(() => undefined) as { error?: { code?: unknown } } | undefined;
      throw new LlmError(typeof body?.error?.code === 'string' ? body.error.code : 'UNAVAILABLE');
    }
    return response;
  };
  const post = async <T>(path: string, body: unknown): Promise<T> => (await call(path, { method: 'POST', body: JSON.stringify(body) })).json() as Promise<T>;
  return {
    ...((import.meta as { env?: Record<string, string | undefined> }).env?.VITE_LINGXI_DATASET_WEB_AGENT === '1' ? {
      async datasetTurn(request: DatasetTurnRequest, signal?: AbortSignal): Promise<DatasetTurn> {
        return (await call('/dataset-turn', { method: 'POST', body: JSON.stringify(request), signal })).json() as Promise<DatasetTurn>;
      },
    } : {}),
    async listProviders() { return ((await (await call('/providers')).json()) as { providers: VaultProvider[] }).providers; },
    async upsertProvider(input) { return (await post<{ provider: VaultProvider }>('/providers', input)).provider; },
    async deleteProvider(id, expectedRevision) { await post('/providers', { action: 'delete', id, ...(expectedRevision === undefined ? {} : { expectedRevision }) }); },
    async testProvider(id, model) { return post<ProviderTestResult>('/providers', { action: 'test', id, ...(model ? { model } : {}) }); },
    async streamChat(turn, onEvent, signal) {
      const response = await call('/chat', { method: 'POST', body: JSON.stringify(turn), signal });
      if (!response.body) throw new LlmError('UNAVAILABLE');
      const reader = response.body.getReader(), decoder = new TextDecoder(), parser = createSseParser();
      let last: ChatEvent | undefined;
      const deliver = (events: ChatEvent[]) => { for (const event of events) { onEvent(event); if (event.type === 'done' || event.type === 'error') last = event; } };
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          deliver(parser.push(decoder.decode(value, { stream: true })));
          if (last) break;
        }
        deliver(parser.push(decoder.decode()));
        deliver(parser.end());
      } catch (error) {
        if ((error as Error)?.name === 'AbortError' || signal?.aborted) throw error;
        last = { type: 'error', code: 'NETWORK' };
        onEvent(last);
      } finally { reader.cancel().catch(() => undefined); }
      if (!last) { last = { type: 'error', code: 'INTERRUPTED' }; onEvent(last); }
      return last;
    },
  };
}
