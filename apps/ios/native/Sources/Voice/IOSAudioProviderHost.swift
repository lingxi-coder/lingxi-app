import Foundation

/// The only cloud transport adapter in the Swift app calls the Rust host.
@MainActor
final class IOSAudioProviderHost: IOSAudioProviderHostDriving {
    let host: IosAudioProviderHost

    init(profilesJSON: String, region: String) throws {
        host = try buildIosAudioProviderHost(
            profilesJson: profilesJSON,
            region: region,
            secureStorage: SecureStorageImpl()
        )
    }

    init(engine: MobileEngineHandle) {
        host = buildIosSessionAudioProviderHost(engine: engine)
    }

    func capabilities(requestJson: String) async throws -> String {
        await host.capabilities(requestJson: requestJson)
    }

    func transcribe(requestJson: String, audio: Data, mimeType: String) async throws -> String {
        await host.transcribe(requestJson: requestJson, audio: audio, mimeType: mimeType)
    }

    func synthesize(requestJson: String, text: String) async throws -> String {
        await host.synthesize(requestJson: requestJson, text: text)
    }

    func cancel(operationId: String) async throws { await host.cancel(operationId: operationId) }
}
