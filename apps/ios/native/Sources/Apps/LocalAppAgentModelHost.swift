import Foundation
import CryptoKit

enum LocalAppAgentModelHostConfiguration {
    enum Decision: Equatable { case build, reuse, restartRequired }

    static func digest(_ configuration: IosEngineLaunchConfigFfi) -> String {
        let identity = [configuration.providerConfig?.providerProfilesJson ?? "", configuration.providerConfig?.routingJson ?? "",
                        configuration.model, configuration.apiBase, configuration.apiKey, configuration.nativeConfigHome,
                        configuration.appSandboxRoot, configuration.mobileLinux?.managedRoot ?? "",
                        configuration.mobileLinux?.brandId ?? "", configuration.mobileLinux?.linuxUser ?? "",
                        configuration.mobileLinux?.configDirName ?? ""].joined(separator: "\u{0}")
        return SHA256.hash(data: Data(identity.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    static func decide(hasHost: Bool, currentDigest: String?, requestedDigest: String) -> Decision {
        guard hasHost else { return .build }
        return currentDigest == requestedDigest ? .reuse : .restartRequired
    }
}

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

    /// Existing tasks and their controls must not replace the executor merely
    /// because a fresh configuration snapshot is temporarily unavailable.
    func ensureAvailable() async throws {
        guard handle == nil else { return }
        try await ensure()
    }

    func ensure() async throws {
        if let pending = building {
            try await complete(pending)
            return try await ensure()
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
            return try await ensure()
        }
        let digest = LocalAppAgentModelHostConfiguration.digest(configuration)
        switch LocalAppAgentModelHostConfiguration.decide(
            hasHost: handle != nil,
            currentDigest: configurationDigest,
            requestedDigest: digest
        ) {
        case .reuse:
            return
        case .restartRequired:
            // Replacing a model host revokes all of its tasks. A task or an
            // approved browser handoff may still be using this host, and a
            // native status snapshot cannot atomically prove otherwise.
            throw LocalAppServiceFailure(
                "AGENT_CONFIGURATION_RESTART_REQUIRED",
                "Model settings changed. Wait for existing Local App Agent tasks to finish or cancel them, then quit and reopen LingXi to use the updated settings."
            )
        case .build:
            break
        }
        let task = Task.detached(priority: .userInitiated) { () throws -> MobileEngineHandle in
            return try await NativeEngineFactory.build(prepared, mediaHost: nil)
        }
        let pending = Building(id: UUID(), digest: digest, task: task)
        building = pending
        try await complete(pending)
        // Settings can change while native construction is in flight. Never
        // dispatch against the stale result, even for the first caller.
        return try await ensure()
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
