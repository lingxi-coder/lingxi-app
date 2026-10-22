import { ManualClock, MemoryRtdb } from '@lingxi/bridge-client/remote';
import { DeviceKeys, MemoryDeviceStore } from '../../src/remote/deviceKey';
import { TunnelMemory } from '../../src/remote/tunnelMemory';
import { ViewerConnection, type ViewerConnectionDeps } from '../../src/remote/viewerConnection';
import { FakeHost } from './fakeHost';
import { FakeWebRtcNetwork } from './fakeWebRtc';

export const UID = 'uid-1';
export const START_TIME = 1_700_000_000_000;

/**
 * Advance fake time in small steps, yielding to the event loop between them so
 * the real asynchronous work (WebCrypto) can finish, until `done()` holds.
 */
export async function drive(
  clock: ManualClock,
  done: () => boolean,
  label: string,
  options: { stepMs?: number; limit?: number } = {},
): Promise<void> {
  const step = options.stepMs ?? 25;
  const limit = options.limit ?? 60_000;
  for (let turn = 0; turn < limit; turn++) {
    if (done()) return;
    await new Promise<void>((resolve) => setImmediate(resolve));
    clock.advance(step);
  }
  throw new Error(`timed out waiting for ${label}`);
}

/** Give promise continuations and WebCrypto jobs a few event-loop turns, without moving fake time. */
export async function settle(turns = 20): Promise<void> {
  for (let turn = 0; turn < turns; turn++) await new Promise<void>((resolve) => setImmediate(resolve));
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
  /** Wait for the connection to reach a phase. */
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
    until: (connection, phase, label = `phase ${phase}`) =>
      drive(clock, () => connection.state.phase === phase, label),
  };
}
