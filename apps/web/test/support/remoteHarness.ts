import { ManualClock, MemoryRtdb } from '@lingxi/bridge-client/remote';
import { DeviceKeys, MemoryDeviceStore } from '../../src/remote/deviceKey';
import { TunnelMemory } from '../../src/remote/tunnelMemory';
import { ViewerConnection, type ViewerConnectionDeps } from '../../src/remote/viewerConnection';
import { FakeHost } from './fakeHost';
import { FakeWebRtcNetwork } from './fakeWebRtc';

export const UID = 'uid-1';
export const START_TIME = 1_700_000_000_000;

/** Wait for asynchronous work using real time, without consuming protocol time. */
export async function waitFor(done: () => boolean, label: string): Promise<void> {
  const deadline = performance.now() + 5_000;
  for (;;) {
    if (done()) return;
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  }
}

const runningClocks = new WeakSet<ManualClock>();

/** Let a timer-dependent operation use real elapsed time, never accelerated time. */
export async function withRealTime<T>(clock: ManualClock, operation: () => Promise<T>): Promise<T> {
  if (runningClocks.has(clock)) throw new Error('a real-time scope already owns this clock');
  runningClocks.add(clock);
  let previous = performance.now();
  let rejectTick!: (error: unknown) => void;
  const failed = new Promise<never>((_resolve, reject) => { rejectTick = reject; });
  const timer = setInterval(() => {
    const elapsed = Math.floor(performance.now() - previous);
    previous += elapsed;
    try { clock.advance(elapsed); } catch (error) { rejectTick(error); }
  }, 1);
  try { return await Promise.race([operation(), failed]); }
  finally {
    clearInterval(timer);
    runningClocks.delete(clock);
  }
}

/** Give promise continuations and WebCrypto jobs a few event-loop turns, without moving fake time. */
export async function settle(turns = 20): Promise<void> {
  for (let turn = 0; turn < turns; turn++) await new Promise<void>((resolve) => setImmediate(resolve));
}

/** Wait for an asynchronous state change without advancing protocol deadlines. */
function waitForPhase(connection: ViewerConnection, phase: ViewerConnection['state']['phase'], label = `phase ${phase}`): Promise<void> {
  return new Promise((resolve, reject) => {
    let finished = false;
    const timeout = setTimeout(() => finish(new Error(`timed out waiting for ${label}`)), 5_000);
    const off = connection.subscribe(check);
    function finish(error?: Error): void {
      if (finished) return;
      finished = true;
      clearTimeout(timeout);
      off();
      if (error) {
        connection.stop();
        reject(error);
      } else resolve();
    }
    function check(state: ViewerConnection['state']): void {
      if (state.phase === phase) finish();
      else if (state.phase === 'failed' || state.phase === 'idle') {
        finish(new Error(`waiting for ${label}: connection ${state.phase}: ${JSON.stringify(state.failure)}`));
      }
    }
    check(connection.state);
  });
}

export interface Harness {
  clock: ManualClock;
  rtdb: MemoryRtdb;
  network: FakeWebRtcNetwork;
  host: FakeHost;
  keys: DeviceKeys;
  tunnelMemory: TunnelMemory;
  /** A viewer connection to the fake host; `deps` override the defaults. */
  connect(deps?: Partial<ViewerConnectionDeps>): ViewerConnection;
  /** Trust this browser on the desktop already, so `initialize` answers `approved`. */
  trustDevice(): Promise<string>;
  /** Wait for a phase without advancing the clock; advance explicit deadlines in the test. */
  until(connection: ViewerConnection, phase: ViewerConnection['state']['phase'], label?: string): Promise<void>;
}

export async function harness(): Promise<Harness> {
  const clock = new ManualClock(START_TIME);
  const rtdb = new MemoryRtdb({ clock });
  const network = new FakeWebRtcNetwork();
  const host = await FakeHost.create({ rtdb, clock, uid: UID, network });
  host.start();
  const keys = new DeviceKeys({ store: new MemoryDeviceStore() });
  const tunnelMemory = new TunnelMemory(clock);
  return {
    clock,
    rtdb,
    network,
    host,
    keys,
    tunnelMemory,
    connect: (deps = {}) =>
      new ViewerConnection({
        uid: UID,
        hostId: host.hostId,
        rtdb,
        keys,
        clock,
        random: () => 1,
        peerConnections: network.factory(),
        tunnelMemory,
        deviceName: 'Test Browser',
        appVersion: 'test',
        ...deps,
      }),
    trustDevice: async () => {
      const identity = await keys.identity();
      host.approvedDevices.add(identity.id);
      return identity.id;
    },
    until: waitForPhase,
  };
}
