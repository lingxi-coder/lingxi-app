import Foundation

/// App-owned dependencies shared by every iOS engine and Remote Control
/// client. The opaque Rust host is constructed once here and passed explicitly
/// to each builder; configuration and native callback objects remain owned by
/// the app.
@MainActor
final class IOSNativeMediaHostComposition {
    let nativeHost: IosNativeMediaHost
    let remoteMediaConfigProvider: IosRemoteMediaConfigProvider

    private var settingsObserver: NSObjectProtocol?

    init(
        audioConfigurationJSON: String,
        remoteMediaConfiguration: RemoteMediaConfigFfi = defaultRemoteMediaConfig()
    ) throws {
        let audio = IOSAudioServiceCallbackAdapter.shared
        let camera = CameraImpl()
        let mediaDevices = IOSMediaDeviceCallbackAdapter.shared
        nativeHost = try IosNativeMediaHost(
            audio: audio,
            camera: camera,
            mediaDevices: mediaDevices,
            mediaAudioConfigurationJson: audioConfigurationJSON
        )
        remoteMediaConfigProvider = IOSRemoteMediaConfigProviderAdapter(
            configuration: remoteMediaConfiguration
        )
        settingsObserver = NotificationCenter.default.addObserver(
            forName: AudioConfigurationStore.didChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in
                guard let self else { return }
                guard let json = AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON else {
                    AudioConfigurationStore.shared.setMediaRuntimeError(
                        "Saved audio settings could not be encoded for the native media host. Reopen Voice settings and try again."
                    )
                    return
                }
                do {
                    try self.nativeHost.updateAudioSettings(mediaAudioConfigurationJson: json)
                    AudioConfigurationStore.shared.setMediaRuntimeError(nil)
                } catch {
                    AudioConfigurationStore.shared.setMediaRuntimeError(
                        "Saved audio settings could not be applied to the native media host. Correct the settings and try again. \(error.localizedDescription)"
                    )
                }
            }
        }
    }

    deinit {
        if let settingsObserver {
            NotificationCenter.default.removeObserver(settingsObserver)
        }
    }

    func updateAudioSessionContext(for engine: MobileEngineHandle) async throws -> MediaAudioSessionBindingFfi {
        try await nativeHost.updateAudioSessionContext(engine: engine)
    }

    func removeAudioSessionContext(sessionID: String, generation: UInt64) {
        nativeHost.removeAudioSessionContext(sessionId: sessionID, generation: generation)
    }
}

/// Freezes an injected product ICE policy for each authenticated offer. The
/// default value comes from the product's remote-control version constants;
/// app compositions can inject TURN servers or relay policy without adding a
/// hidden SDK/runtime global.
final class IOSRemoteMediaConfigProviderAdapter: IosRemoteMediaConfigProvider, @unchecked Sendable {
    private let configuration: RemoteMediaConfigFfi

    init(configuration: RemoteMediaConfigFfi) {
        self.configuration = configuration
    }

    func resolve(request _: RemoteMediaConfigRequestFfi) async throws -> RemoteMediaConfigFfi {
        configuration
    }
}
