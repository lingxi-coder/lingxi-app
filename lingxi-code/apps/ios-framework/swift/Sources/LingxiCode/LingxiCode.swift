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
    public static func make(appSandboxRoot: String) throws -> LingxiCodeEngine {
        let impls = PlatformImpls(
            camera: IosCameraImpl(),
            voice: IosVoiceImpl(),
            share: IosShareImpl(),
            appSandboxRoot: appSandboxRoot
        )
        let handle = try buildMobileEngine(impls: impls)
        return LingxiCodeEngine(handle: handle)
    }

    /// Number of builtin mobile skills the engine assembled.
    public var skillCount: UInt32 { handle.skillCount() }
}
