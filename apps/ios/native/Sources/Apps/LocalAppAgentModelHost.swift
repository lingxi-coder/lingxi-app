import Foundation
import CryptoKit

/// Owns model services independently of conversation navigation and history.
/// The private handle never receives prompts or application approval commands.
actor LocalAppAgentModelHost {
    static let shared = LocalAppAgentModelHost()
    private var handle: MobileEngineHandle?
    private var building: Task<MobileEngineHandle, Error>?
    private var configurationDigest: String?

    func ensure() async throws {
        if let building {
            _ = try await building.value
            return
        }
        let configuration: IosEngineLaunchConfigFfi = try await MainActor.run {
            let snapshot = ProviderRepository.shared.makeLaunchSnapshot()
            let metadata = LXISHRuntimeBundleMetadata.current()
            let workspace = try MobileUserStorage.prepareProjectsDirectory()
            let sandbox = ConversationSourceFactory.appSandboxRoot()
            return IosEngineLaunchConfigFfi(
                appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
                appAgentHost: true,
                apiBase: Keychain.get(.apiBase) ?? "https://api.anthropic.com", apiKey: Keychain.get(.apiKey) ?? "",
                model: snapshot.defaultModelID ?? "", sessionMode: .code, visionDelegationEnabled: false,
                appSandboxRoot: sandbox, projectCwd: nil,
                providerConfig: IosProviderConfigFfi(providerProfilesJson: snapshot.providerProfilesJSON, routingJson: snapshot.routingJSON),
                mobileLinux: IosMobileLinuxConfigFfi(mode: .mobileLinux, managedRoot: LXISHDefaultWorkspace.managedRootPath(), workspaceHostPath: workspace.path,
                    stableWorkspaceId: LXISHDefaultWorkspace.stableID(), abi: "arm64", rootfsVersion: metadata.rootfsVersion, archiveSha256: metadata.archiveSha256,
                    authorizationFile: LXISHRuntimeBundleResources.authorizationManifestURL()?.path, appSandboxRoot: sandbox),
                physicalMemoryBytes: ProcessInfo.processInfo.physicalMemory, hostEnvironment: nil)
        }
        // The actor can reenter while the global snapshot is read.
        if let building { _ = try await building.value; return }
        let identity = [configuration.providerConfig?.providerProfilesJson ?? "", configuration.providerConfig?.routingJson ?? "",
                        configuration.model, configuration.apiBase, configuration.apiKey].joined(separator: "\u{0}")
        let digest = SHA256.hash(data: Data(identity.utf8)).map { String(format: "%02x", $0) }.joined()
        if handle != nil && configurationDigest == digest { return }
        let task = Task.detached(priority: .userInitiated) { () throws -> MobileEngineHandle in
        let audio = await MainActor.run { IOSAudioServiceCallbackAdapter.shared }
            return try await withCheckedThrowingContinuation { continuation in
                let thread = Thread {
                    autoreleasepool {
                        continuation.resume(with: Result {
                            let permissions = AppAgentBootstrapPermissions()
                            let handle = try buildIosEngineWithConfig(config: configuration, listener: AppAgentBootstrapListener(), audio: audio,
                                camera: CameraImpl(), share: ShareImpl(), notifications: NotificationImpl(), clipboard: ClipboardImpl(),
                                permissions: permissions, secureStorage: SecureStorageImpl(), deviceControl: DeviceControlImpl())
                            permissions.attach(handle)
                            return handle
                        })
                    }
                }
                thread.name = "Local App model host"; thread.qualityOfService = .userInitiated
                thread.stackSize = 8 * 1024 * 1024; thread.start()
            }
        }
        building = task
        do { handle = try await task.value; configurationDigest = digest; building = nil }
        catch { building = nil; throw error }
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
