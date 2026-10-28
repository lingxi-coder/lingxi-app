import Foundation
import CryptoKit

/// Owns model services independently of conversation navigation and history.
/// The private handle never receives prompts or application approval commands.
actor LocalAppAgentModelHost {
    static let shared = LocalAppAgentModelHost()
    #if LINGXI_FULL
    static let usesMobileLinuxRuntime = true
    #else
    static let usesMobileLinuxRuntime = false
    #endif
    private var handle: MobileEngineHandle?
    private struct Building {
        let id: UUID
        let digest: String
        let task: Task<MobileEngineHandle, Error>
    }
    private var building: Building?
    private var configurationDigest: String?

    func ensure(mediaHostComposition: IOSNativeMediaHostComposition) async throws {
        if let pending = building {
            try await complete(pending)
            return try await ensure(mediaHostComposition: mediaHostComposition)
        }
        let prepared = try await MainActor.run {
            let snapshot = ProviderRepository.shared.makeLaunchSnapshot()
            let sandbox = ConversationSourceFactory.appSandboxRoot()
            return try NativeEngineFactory.prepare(.appAgent(appSandboxRoot: sandbox, snapshot: snapshot))
        }
        let configuration = prepared.configuration
        // The actor can reenter while the global snapshot is read.
        if let pending = building {
            try await complete(pending)
            return try await ensure(mediaHostComposition: mediaHostComposition)
        }
        let identity = [configuration.providerConfig?.providerProfilesJson ?? "", configuration.providerConfig?.routingJson ?? "",
                        configuration.apiBase, configuration.apiKey, configuration.nativeConfigHome,
                        configuration.appSandboxRoot, configuration.mobileLinux?.managedRoot ?? "",
                        configuration.mobileLinux?.brandId ?? "", configuration.mobileLinux?.linuxUser ?? "",
                        configuration.mobileLinux?.configDirName ?? ""].joined(separator: "\u{0}")
        let digest = SHA256.hash(data: Data(identity.utf8)).map { String(format: "%02x", $0) }.joined()
        if handle != nil && configurationDigest == digest { return }
        let task = Task.detached(priority: .userInitiated) { () throws -> MobileEngineHandle in
            let mediaHost = await MainActor.run { mediaHostComposition.nativeHost }
            return try await NativeEngineFactory.build(prepared, mediaHost: mediaHost)
        }
        let pending = Building(id: UUID(), digest: digest, task: task)
        building = pending
        try await complete(pending)
        // Settings can change while native construction is in flight. Never
        // dispatch against the stale result, even for the first caller.
        return try await ensure(mediaHostComposition: mediaHostComposition)
    }

    private func complete(_ pending: Building) async throws {
        do {
            let completed = try await pending.task.value
            if building?.id == pending.id {
                handle = completed; configurationDigest = pending.digest; building = nil
            }
        } catch {
            if building?.id == pending.id { building = nil }
            throw error
        }
    }
}
