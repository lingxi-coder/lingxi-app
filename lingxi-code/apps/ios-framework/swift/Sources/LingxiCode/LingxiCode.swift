// M8-P12 skeleton — the public Swift entry point.
//
// `LingxiCodeBindings` exposes the UniFFI-generated symbols: the
// `MobileEngineHandle` object, the `PlatformImpls` record, the
// `buildMobileEngine(_:)` function, and the `CameraControl` / `VoiceRecorder` /
// `SharingService` *callback-interface protocols* that Swift implements (see
// CameraImpl.swift etc.). Rust calls back into those Swift objects — that is
// the bidirectional UniFFI seam.
import Foundation
import LingxiCodeBindings

/// Ergonomic wrapper over the UniFFI `MobileEngineHandle`.
public final class LingxiCodeEngine {
  private let handle: MobileEngineHandle

  private init(handle: MobileEngineHandle) {
    self.handle = handle
  }

  /// Construct the engine, wiring the native iOS capability implementations.
  public static func make(
    appSandboxRoot: String,
    mobileLinux: IosMobileLinuxConfigFfi? = nil
  ) throws -> LingxiCodeEngine {
    let impls = PlatformImpls(
      camera: IosCameraImpl(),
      voice: IosVoiceImpl(),
      share: IosShareImpl(),
      appSandboxRoot: appSandboxRoot,
      mobileLinux: mobileLinux
    )
    let handle = try buildMobileEngine(impls: impls)
    return LingxiCodeEngine(handle: handle)
  }

  /// Number of builtin mobile skills the engine assembled.
  public var skillCount: UInt32 { handle.skillCount() }
}

public final class LingxiMobileLinux {
  public let config: IosMobileLinuxConfigFfi
  private let handle: IosMobileLinuxRuntimeHandle

  private init(config: IosMobileLinuxConfigFfi, handle: IosMobileLinuxRuntimeHandle) {
    self.config = config
    self.handle = handle
  }

  public static func probe(config: IosMobileLinuxConfigFfi? = nil) -> MobileLinuxCapabilityFfi {
    probeIosMobileLinux(config: config)
  }

  public static func status(config: IosMobileLinuxConfigFfi? = nil) -> MobileLinuxStatusFfi {
    iosMobileLinuxStatus(config: config)
  }

  public static func verify(config: IosMobileLinuxConfigFfi? = nil) -> MobileLinuxStatusFfi {
    verifyIosMobileLinux(config: config)
  }

  public static func repair(config: IosMobileLinuxConfigFfi? = nil) -> MobileLinuxStatusFfi {
    repairIosMobileLinux(config: config)
  }

  public static func reset(config: IosMobileLinuxConfigFfi? = nil) -> MobileLinuxStatusFfi {
    resetIosMobileLinux(config: config)
  }

  public static func make(config: IosMobileLinuxConfigFfi) throws -> LingxiMobileLinux {
    let handle = try createIosMobileLinuxRuntime(config: config)
    return LingxiMobileLinux(config: config, handle: handle)
  }

  public func capability() async -> MobileLinuxCapabilityFfi {
    await handle.capability()
  }

  public func boot() async throws -> MobileLinuxStatusFfi {
    try await handle.boot()
  }

  public func shutdown() async throws -> MobileLinuxStatusFfi {
    try await handle.shutdown()
  }

  public func status() async throws -> MobileLinuxStatusFfi {
    try await handle.status()
  }

  public func verifyRootfs() async throws -> MobileLinuxStatusFfi {
    try await handle.verifyRootfs()
  }

  public func repairRootfs() async throws -> MobileLinuxStatusFfi {
    try await handle.repairRootfs()
  }

  public func resetRootfs() async throws -> MobileLinuxStatusFfi {
    try await handle.resetRootfs()
  }

  public func run(_ request: MobileLinuxCommandRequestFfi) async throws -> MobileLinuxCommandResultFfi {
    try await handle.runCommand(request: request)
  }

  public func spawn(_ request: MobileLinuxCommandRequestFfi) async throws -> MobileLinuxTaskFfi {
    try await handle.spawnTask(request: request)
  }

  public func listTasks() async throws -> [MobileLinuxTaskFfi] {
    try await handle.listTasks()
  }

  public func taskStatus(_ taskID: String) async throws -> MobileLinuxTaskFfi? {
    try await handle.taskStatus(taskId: taskID)
  }

  public func readEvents(afterSequence: UInt64? = nil, limit: UInt32? = nil) async throws -> [MobileLinuxStreamEventFfi] {
    try await handle.readEvents(afterSequence: afterSequence, limit: limit)
  }

  public func killTask(_ taskID: String) async throws -> MobileLinuxTaskFfi {
    try await handle.killTask(taskId: taskID)
  }

  public func openPty(_ request: MobileLinuxPtyOpenRequestFfi) async throws -> MobileLinuxPtySessionFfi {
    try await handle.openPty(request: request)
  }

  public func writePty(sessionID: String, data: [UInt8]) async throws {
    try await handle.writePty(sessionId: sessionID, data: data)
  }

  public func resizePty(sessionID: String, cols: UInt16, rows: UInt16) async throws {
    try await handle.resizePty(sessionId: sessionID, cols: cols, rows: rows)
  }

  public func closePty(sessionID: String) async throws {
    try await handle.closePty(sessionId: sessionID)
  }

  public func configureMounts(_ mounts: [MobileLinuxMountSpecFfi]) async throws -> MobileLinuxStatusFfi {
    try await handle.configureMounts(mounts: mounts)
  }
}
