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
        let configuration: IosEngineLaunchConfigFfi = try await MainActor.run {
            let snapshot = ProviderRepository.shared.makeLaunchSnapshot()
            let directories = MobileUserStorage.directories()
            let sandbox = ConversationSourceFactory.appSandboxRoot()
            let mobileLinux: IosMobileLinuxConfigFfi?
            #if LINGXI_FULL
            let metadata = LXISHRuntimeBundleMetadata.current()
            let workspace = try MobileUserStorage.prepareProjectsDirectory()
            mobileLinux = IosMobileLinuxConfigFfi(
                mode: .mobileLinux,
                brandId: directories.brandId,
                linuxUser: directories.linuxUser,
                configDirName: directories.configDirName,
                managedRoot: LXISHDefaultWorkspace.managedRootPath(),
                workspaceHostPath: workspace.path,
                stableWorkspaceId: LXISHDefaultWorkspace.stableID(),
                abi: "arm64",
                rootfsVersion: metadata.rootfsVersion,
                archiveSha256: metadata.archiveSha256,
                authorizationFile: LXISHRuntimeBundleResources.authorizationManifestURL()?.path,
                appSandboxRoot: sandbox
            )
            #else
            // Store App Agents use the provider-only host. They share the
            // native Local App config directory, without preparing Linux
            // workspace/rootfs resources or registering Linux-only tools.
            mobileLinux = nil
            #endif
            let apiBase: String?
            let apiKey: String?
            do {
                apiBase = try Keychain.getOrThrow(.apiBase)
                apiKey = try Keychain.getOrThrow(.apiKey)
            } catch {
                throw LocalAppServiceFailure(
                    "CREDENTIAL_STORAGE_UNAVAILABLE",
                    "The model's secure settings are temporarily unavailable. Try again when the device is unlocked."
                )
            }
            return IosEngineLaunchConfigFfi(
                appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
                appAgentHost: true,
                brandId: directories.brandId,
                linuxUser: directories.linuxUser,
                configDirName: directories.configDirName,
                nativeConfigHome: directories.configHome,
                apiBase: apiBase ?? "https://api.anthropic.com", apiKey: apiKey ?? "",
                model: snapshot.defaultModelID ?? "", sessionMode: .code, visionDelegationEnabled: false,
                appSandboxRoot: sandbox,
                projectCwd: nil,
                projectAdditionalDirectories: [],
                projectMountGrants: [],
                providerConfig: IosProviderConfigFfi(providerProfilesJson: snapshot.providerProfilesJSON, routingJson: snapshot.routingJSON),
                mobileLinux: mobileLinux,
                physicalMemoryBytes: ProcessInfo.processInfo.physicalMemory, hostEnvironment: nil)
        }
        // The actor can reenter while the global snapshot is read.
        if let pending = building {
            try await complete(pending)
            return try await ensure()
        }
        let identity = [configuration.providerConfig?.providerProfilesJson ?? "", configuration.providerConfig?.routingJson ?? "",
                        configuration.apiBase, configuration.apiKey, configuration.nativeConfigHome,
                        configuration.appSandboxRoot, configuration.mobileLinux?.managedRoot ?? "",
                        configuration.mobileLinux?.brandId ?? "", configuration.mobileLinux?.linuxUser ?? "",
                        configuration.mobileLinux?.configDirName ?? ""].joined(separator: "\u{0}")
        let digest = SHA256.hash(data: Data(identity.utf8)).map { String(format: "%02x", $0) }.joined()
        if handle != nil && configurationDigest == digest { return }
        let task = Task.detached(priority: .userInitiated) { () throws -> MobileEngineHandle in
            return try await withCheckedThrowingContinuation { continuation in
                let thread = Thread {
                    autoreleasepool {
                        continuation.resume(with: Result {
                            let permissions = AppAgentBootstrapPermissions()
                            let handle = try buildIosEngineWithConfig(
                                config: configuration,
                                listener: AppAgentBootstrapListener(),
                                mediaHost: nil,
                                share: ShareImpl(),
                                notifications: NotificationImpl(),
                                clipboard: ClipboardImpl(),
                                permissions: permissions,
                                secureStorage: SecureStorageImpl(),
                                deviceControl: DeviceControlImpl()
                            )
                            permissions.attach(handle)
                            return handle
                        })
                    }
                }
                thread.name = "Local App model host"; thread.qualityOfService = .userInitiated
                thread.stackSize = 8 * 1024 * 1024; thread.start()
            }
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
private final class AppAgentBootstrapListener: IosEventListener, @unchecked Sendable {
    func onEvent(event: ClientEvent) async {}
    func onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) async {}
}
private final class AppAgentBootstrapPermissions: IosPermissionSink, @unchecked Sendable {
    private let lock = NSLock()
    private weak var handle: MobileEngineHandle?
    func attach(_ handle: MobileEngineHandle) { lock.withLock { self.handle = handle } }
    func onRequest(request: PermissionRequest) async {
        let current = lock.withLock { handle }
        try? await current?.submit(command: .denyPermission(requestId: request.requestId))
    }
}
