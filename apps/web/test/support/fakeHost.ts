import {
  CLIENT_PROTOCOL_MAJOR,
  MessageTransport,
  REMOTE_PROTOCOL_MAJOR,
  RemoteRpcError,
  RpcPeer,
  RtdbMirror,
  SERVER_TIMESTAMP,
  TunnelLink,
  answerPath,
  b64uDecode,
  b64uEncode,
  deriveTunnelKeys,
  downPath,
  generateEphemeralKey,
  generateIdentityKey,
  hostPath,
  idFromKey,
  pairingCodeFor,
  parseFrameKey,
  randomBytes,
  signAnswer,
  toHostPath,
  upPath,
  verifyOffer,
  x25519,
  type AnswerDraft,
  type AnswerRejectReason,
  type FragmentLink,
  type IdentityKey,
  type ManualClock,
  type MemoryRtdb,
  type OfferRecord,
  type RemoteHostInfo,
  type RemoteSessionSummary,
  type TransportKind,
} from '@lingxi/bridge-client/remote';
import { DataChannelLink } from '../../src/remote/peerLink';
import { FAKE_SDP_ANSWER, type FakeWebRtcNetwork } from './fakeWebRtc';

export interface FakeHostOptions {
  rtdb: MemoryRtdb;
  clock: ManualClock;
  uid: string;
  network: FakeWebRtcNetwork;
  info?: Partial<RemoteHostInfo>;
}

/** The host's side of one viewer connection. */
export class HostConnection {
  rpc!: RpcPeer;
  link!: FragmentLink;
  approved = false;
  pairingCode = '';
  /** While set, nothing the host sends reaches the viewer (a hung desktop). */
  silent = false;
  readonly closes: string[] = [];

  constructor(
    readonly connId: string,
    readonly offer: OfferRecord,
    readonly transport: TransportKind,
  ) {}

  notify(method: string, params?: unknown): Promise<void> {
    return this.rpc.notify(method, params);
  }
}

export type HostRequestHandler = (method: string, params: unknown, connection: HostConnection) => unknown;

/**
 * A desktop built from the shared primitives, standing on the other side of a
 * `MemoryRtdb`: it publishes its host record, verifies offers, signs answers,
 * and speaks the RPC protocol over a fake DataChannel or the real tunnel.
 * Its public fields are the knobs a test turns to make the viewer's life hard.
 */
export class FakeHost {
  static async create(options: FakeHostOptions): Promise<FakeHost> {
    const key = await generateIdentityKey({ extractable: false });
    const host = new FakeHost(options, key, await idFromKey(key.publicKey));
    await host.publish();
    return host;
  }

  readonly info: RemoteHostInfo;
  readonly connections = new Map<string, HostConnection>();
  /** Every offer that verified, in arrival order. */
  readonly offers: Array<{ connId: string; offer: OfferRecord }> = [];
  readonly requests: Array<{ method: string; params: unknown; connId: string }> = [];
  readonly approvedDevices = new Set<string>();
  readonly revokedDevices = new Set<string>();
  sessions: RemoteSessionSummary[] = [];

  /** `'silent'`: never answers an offer. */
  mode: 'answer' | 'silent' = 'answer';
  /** Answer every offer with `rejected` and this reason. */
  rejectWith: AnswerRejectReason | undefined;
  /** Answer a direct offer with this transport instead (the host-side downgrade to the tunnel). */
  transportChoice: TransportKind | undefined;
  /** Sign answers with this key instead of the host key. */
  signingKey: IdentityKey;
  /** Report this pairing code instead of the real one. */
  pairingCodeOverride: string | undefined;
  /** Handle requests beyond the handshake; the default answers an empty result. */
  requestHandler: HostRequestHandler | undefined;

  private readonly mirror = new RtdbMirror();
  private subscription: { close(): void } | undefined;

  private constructor(
    private readonly options: FakeHostOptions,
    readonly hostKey: IdentityKey,
    readonly hostId: string,
  ) {
    this.signingKey = hostKey;
    this.info = { name: 'Studio Mac', platform: 'darwin', app_version: '1.2.3', ...options.info };
  }

  get hostKeyB64(): string {
    return b64uEncode(this.hostKey.publicKey);
  }

  /** Write `hosts/{host_id}`; `host_key` may be overridden to model a wrong advertisement. */
  async publish(advertisedKey: string = this.hostKeyB64): Promise<void> {
    await this.options.rtdb.put(hostPath(this.options.uid, this.hostId), {
      v: 1,
      name: this.info.name,
      platform: this.info.platform,
      app_version: this.info.app_version,
      client_protocol_version: CLIENT_PROTOCOL_MAJOR,
      host_key: advertisedKey,
      online: true,
      last_seen: SERVER_TIMESTAMP,
      started_at: SERVER_TIMESTAMP,
    });
  }

  /** Start reading the inbox. */
  start(): void {
    const { rtdb, uid } = this.options;
    this.subscription = rtdb.listen(toHostPath(uid, this.hostId), (event) => {
      const changes = this.mirror.apply(event);
      for (const change of changes.at(2)) {
        const [connId, kind] = change.path;
        if (kind === 'offer' && change.kind !== 'removed' && connId !== undefined && !this.connections.has(connId)) {
          void this.onOffer(connId, change.value);
        }
      }
      for (const change of changes.at(3)) {
        const [connId, side, key] = change.path;
        if (side !== 'up' || change.kind === 'removed' || connId === undefined || key === undefined) continue;
        const seq = parseFrameKey(key);
        const link = this.connections.get(connId)?.link;
        if (seq !== null && link instanceof TunnelLink) link.deliverFrame(seq, change.value);
      }
    });
  }

  stop(): void {
    this.subscription?.close();
    for (const connection of this.connections.values()) connection.rpc?.close('host_stopped');
  }

  /** The user clicked Allow on the desktop. */
  approve(deviceId: string): void {
    this.approvedDevices.add(deviceId);
    for (const connection of this.connections.values()) {
      if (connection.offer.device.id === deviceId && !connection.approved) {
        connection.approved = true;
        void connection.notify('auth.approved', { host: this.info });
      }
    }
  }

  /** The user clicked Deny. */
  deny(deviceId: string): void {
    for (const connection of this.connections.values()) {
      if (connection.offer.device.id === deviceId) {
        void connection.notify('auth.denied', { reason: 'denied' }).then(() => connection.rpc.close('denied'));
      }
    }
  }

  private async onOffer(connId: string, record: unknown): Promise<void> {
    const { rtdb, uid, clock } = this.options;
    const ctx = { uid, hostId: this.hostId, connId };
    const verified = await verifyOffer({ ...ctx, record, now: clock.now() });
    if (!verified.ok) return;
    const offer = verified.value;
    this.offers.push({ connId, offer });
    if (this.mode === 'silent') return;

    const draftBase = { v: 1 as const, host: { id: this.hostId, key: this.hostKeyB64 }, nonce: b64uEncode(randomBytes(16)) };
    const reason = this.rejectWith ?? (this.revokedDevices.has(offer.device.id) ? 'revoked' : undefined);
    if (reason !== undefined) {
      const draft: AnswerDraft = { ...draftBase, status: 'rejected', reason };
      await rtdb.put(answerPath(uid, this.hostId, connId), await signAnswer(ctx, this.signingKey.privateKey, offer.nonce, draft));
      return;
    }

    const transport: TransportKind = offer.transport === 'webrtc' ? (this.transportChoice ?? 'webrtc') : 'tunnel';
    const connection = new HostConnection(connId, offer, transport);
    connection.approved = this.approvedDevices.has(offer.device.id);
    this.connections.set(connId, connection);

    let draft: AnswerDraft;
    if (transport === 'webrtc') {
      const channel = this.options.network.acceptOffer(offer.sdp);
      if (channel === undefined) return;
      connection.link = new DataChannelLink(channel);
      draft = { ...draftBase, status: 'accepted', transport, sdp: FAKE_SDP_ANSWER };
    } else {
      const eph = await generateEphemeralKey();
      const shared = await x25519(eph.privateKey, b64uDecode(offer.eph));
      const keys = await deriveTunnelKeys(shared, connId);
      connection.link = new TunnelLink({
        rtdb,
        connId,
        sendPath: downPath(uid, this.hostId, connId),
        sendDir: 'down',
        sendKey: keys.down,
        recvPath: upPath(uid, this.hostId, connId),
        recvDir: 'up',
        recvKey: keys.up,
        subscribe: false,
        clock,
      });
      draft = { ...draftBase, status: 'accepted', transport, eph: b64uEncode(eph.publicKey) };
    }

    const link = connection.link;
    const gated: FragmentLink = {
      sendFragment: (fragment) => {
        if (!connection.silent) link.sendFragment(fragment);
      },
      onFragment: (callback) => link.onFragment(callback),
      onClose: (callback) => link.onClose(callback),
      close: (closeReason) => link.close(closeReason),
      whenWritable: link.whenWritable?.bind(link),
      maxFragmentBytes: link.maxFragmentBytes,
    };
    const transportLayer = new MessageTransport(gated);
    transportLayer.onClose((closeReason) => connection.closes.push(closeReason));
    connection.rpc = new RpcPeer(transportLayer, { clock });
    connection.rpc.onRequest((method, params) => this.handle(connection, method, params));
    connection.rpc.startKeepalive(transport === 'webrtc' ? 'datachannel' : 'tunnel');

    connection.pairingCode = await pairingCodeFor(offer, { host: draftBase.host, nonce: draftBase.nonce });
    await rtdb.put(answerPath(uid, this.hostId, connId), await signAnswer(ctx, this.signingKey.privateKey, offer.nonce, draft));
  }

  private async handle(connection: HostConnection, method: string, params: unknown): Promise<unknown> {
    this.requests.push({ method, params, connId: connection.connId });
    if (method === 'initialize') {
      const protocol = (params as { protocol?: number } | null)?.protocol;
      if (protocol !== REMOTE_PROTOCOL_MAJOR) throw new RemoteRpcError('upgrade_required');
      if (connection.approved) return { status: 'approved', host: this.info };
      return { status: 'pending_approval', pairing_code: this.pairingCodeOverride ?? connection.pairingCode };
    }
    if (!connection.approved) throw new RemoteRpcError('not_approved');
    if (this.requestHandler !== undefined) return this.requestHandler(method, params, connection);
    if (method === 'sessions.list' || method === 'sessions.watch') return { sessions: this.sessions };
    return {};
  }
}
