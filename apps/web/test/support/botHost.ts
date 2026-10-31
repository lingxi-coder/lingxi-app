import { DATA_CHANNEL_DEAD_MS, RemoteRpcError, type ManualClock, type RemoteBotSummary, type RemoteSessionSummary } from '@lingxi/bridge-client/remote';
import { RemoteClient } from '../../src/remote/remoteClient';
import type { ViewerConnection } from '../../src/remote/viewerConnection';
import type { FakeHost, HostConnection } from './fakeHost';
import { waitFor, withRealTime, settle, type Harness } from './remoteHarness';

export const bot = (id: string, change: Partial<RemoteBotSummary> = {}): RemoteBotSummary => ({
  bot_id: id,
  name: `Bot ${id}`,
  description: `${id} helps`,
  status: 'idle',
  waiting_threads: 0,
  ...change,
});

export const thread = (id: string, botId: string, change: Partial<RemoteSessionSummary> = {}): RemoteSessionSummary => ({
  session_id: id,
  title: `Chat ${id}`,
  project_label: `Bot ${botId}`,
  status: 'idle',
  viewers: 0,
  bot_id: botId,
  ...change,
});

const isBotMethod = (method: string): boolean => /^bots?\./u.test(method);

/**
 * A desktop that has Bots, scripted: the lists it answers with, the idempotent
 * `bot.thread.create` of the real one, and the ways a test makes it misbehave.
 */
export class BotHost {
  /** `false`: a desktop from before Bots, whose answer to every Bot method is the unknown-method error. */
  supported = true;
  bots: RemoteBotSummary[] = [];
  sessions: RemoteSessionSummary[] = [];
  readonly threads = new Map<string, RemoteSessionSummary[]>();
  /** The error to answer a method with (Bot methods: only where `supported`). */
  readonly failWith = new Map<string, RemoteRpcError>();
  /** The chats made so far, by `client_request_id`: a repeat is given the first one, as the real desktop does. */
  readonly created = new Map<string, string>();
  /** The next `times` requests of this method are done, and then the link they came over is ended: no answer arrives. */
  dropping: { method: string; times: number } | undefined;
  /** While set, `bot.thread.create` and `bot.threads.list` wait for it before they answer. */
  holdCreate: Promise<void> | undefined;
  holdThreads: Promise<void> | undefined;

  constructor(private readonly host: FakeHost) {
    host.requestHandler = (method, params, connection) => this.handle(method, (params ?? {}) as Record<string, any>, connection);
  }

  /** The requests received so far for a method. */
  requests(method: string): Array<{ params: any; connId: string }> {
    return this.host.requests.filter((request) => request.method === method).map(({ params, connId }) => ({ params, connId }));
  }

  /** Every Bot request received so far, in order. */
  botRequests(): string[] {
    return this.host.requests.filter((request) => isBotMethod(request.method)).map((request) => request.method);
  }

  /** Tell the newest connection the Bots as they are now, as the desktop does when one changes. */
  async announce(): Promise<void> {
    const connection = [...this.host.connections.values()].at(-1);
    await connection?.notify('bots.changed', { bots: this.bots });
  }

  private async handle(method: string, params: Record<string, any>, connection: HostConnection): Promise<unknown> {
    if (isBotMethod(method) && !this.supported) throw new RemoteRpcError('invalid_params', `unknown method ${method}`);
    const failure = this.failWith.get(method);
    if (failure !== undefined) throw failure;
    switch (method) {
      case 'sessions.list':
      case 'sessions.watch':
        return { sessions: this.sessions };
      case 'bots.list':
      case 'bots.watch':
        return { bots: this.bots };
      case 'bot.threads.list':
        await this.holdThreads;
        return { sessions: this.threads.get(params['bot_id']) ?? [] };
      case 'bot.thread.create': {
        const requestId = String(params['client_request_id']);
        let sessionId = this.created.get(requestId);
        if (sessionId === undefined) {
          sessionId = `chat-${this.created.size + 1}`;
          this.created.set(requestId, sessionId);
        }
        if (this.dropping?.method === method && this.dropping.times > 0) {
          this.dropping.times -= 1;
          connection.rpc.close('dropped');
          return { session_id: sessionId };
        }
        await this.holdCreate;
        return { session_id: sessionId };
      }
      case 'session.attach':
        return { mode: 'snapshot', epoch: 'e1' };
      default:
        return {};
    }
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Driving a client over a connection to the scripted desktop
// ─────────────────────────────────────────────────────────────────────────────

export async function open(h: Harness): Promise<{ connection: ViewerConnection; client: RemoteClient }> {
  await h.trustDevice();
  const connection = h.connect();
  const client = new RemoteClient(connection, { clock: h.clock });
  connection.start();
  await h.until(connection, 'ready');
  return { connection, client };
}

/** Let what is on its way arrive: a request and its answer take a few turns of the event loop. */
export async function quiet(h: Harness): Promise<void> {
  for (let turn = 0; turn < 10; turn++) {
    await settle();
    h.clock.advance(25);
  }
}

/** Wait for Bot replies while protocol time follows real elapsed time. */
export function drive(clock: ManualClock, done: () => boolean, label: string): Promise<void> {
  return withRealTime(clock, () => waitFor(done, label));
}

export const answered = (h: Harness, client: RemoteClient) => drive(h.clock, () => client.getState().bots !== undefined, 'the answer about Bots');

/** The desktop sends the Bots as they are now, and the client has taken them in. */
export async function announce(h: Harness, client: RemoteClient, host: BotHost): Promise<void> {
  const before = client.getState().bots;
  await host.announce();
  await drive(h.clock, () => client.getState().bots !== before, 'bots.changed');
  await quiet(h);
}

/** The desktop goes quiet; the keepalive ends the link, and the viewer dials again. */
export async function reconnect(h: Harness, connection: ViewerConnection): Promise<void> {
  [...h.host.connections.values()].at(-1)!.silent = true;
  h.clock.advance(DATA_CHANNEL_DEAD_MS);
  await h.until(connection, 'reconnecting', 'the keepalive timeout');
  const retryAt = connection.state.retryAt;
  if (retryAt !== undefined) h.clock.advance(Math.max(0, retryAt - h.clock.now()));
  await h.until(connection, 'ready');
}
