import Foundation

#if canImport(harness_runtimeFFI)

/// Composes a session engine around the app-owned media host. Each scenario
/// spells out its own credentials, workspace, callbacks, and capabilities;
/// only the common FFI construction and native services are shared.
enum NativeEngineFactory {
    typealias HandleBuilder = (
        _ config: IosEngineLaunchConfigFfi,
        _ listener: IosEventListener,
        _ mediaHost: IosNativeMediaHost?,
        _ permissions: IosPermissionSink
    ) throws -> MobileEngineHandle

    enum Scenario {
        case interactive(config: EngineConfig, listener: IosEventListener, permissions: IosPermissionSink)
        case scheduled(
            appSandboxRoot: String,
            scope: CronScope,
            task: CronTaskRecord?,
            snapshot: ProviderLaunchSnapshot,
            runtime: TerminalRuntimeConfig?
        )
        case appAgent(appSandboxRoot: String, snapshot: ProviderLaunchSnapshot)
    }

    fileprivate enum Policy {
        case interactive(listener: IosEventListener, permissions: IosPermissionSink)
        case scheduled
        case appAgent
    }

    struct Prepared {
        let configuration: IosEngineLaunchConfigFfi
        fileprivate let policy: Policy
    }

    private struct LaunchSettings {
        let appAgentHost: Bool
        let apiBase: String
        let apiKey: String
        let model: String
        let sessionMode: SessionModeDto
        let visionDelegationEnabled: Bool
        let appSandboxRoot: String
        let projectCwd: String?
        let projectAdditionalDirectories: [String]
        let projectMountGrants: [String]
        let providerConfig: IosProviderConfigFfi?
        let mobileLinux: IosMobileLinuxConfigFfi?
        let hostEnvironment: IosHostEnvironmentFfi?
    }

    /// Configuration is captured on the main actor along with provider and
    /// device state. Building the engine itself always happens off-main.
    @MainActor
    static func prepare(_ scenario: Scenario) throws -> Prepared {
        let directories = MobileUserStorage.directories()
        let settings: LaunchSettings
        let policy: Policy
        switch scenario {
        case let .interactive(config, listener, permissions):
            settings = LaunchSettings(
                appAgentHost: false,
                apiBase: config.apiBase,
                apiKey: config.apiKey,
                model: config.model,
                sessionMode: config.sessionMode.dto,
                visionDelegationEnabled: config.visionDelegationEnabled,
                appSandboxRoot: config.appSandboxRoot,
                projectCwd: config.projectCwd,
                projectAdditionalDirectories: config.additionalWorkspaceDirectories,
                projectMountGrants: config.projectMountGrants,
                providerConfig: config.providerProfilesJson.map {
                    IosProviderConfigFfi(providerProfilesJson: $0, routingJson: config.providerRoutingJson)
                },
                mobileLinux: config.mobileLinux.map {
                    mobileLinuxConfiguration($0, appSandboxRoot: config.appSandboxRoot)
                },
                hostEnvironment: makeIosHostEnvironment(launchMode: .interactive)
            )
            policy = .interactive(listener: listener, permissions: permissions)

        case let .scheduled(appSandboxRoot, scope, task, snapshot, runtime):
            settings = LaunchSettings(
                appAgentHost: false,
                apiBase: Keychain.get(.apiBase) ?? "https://api.anthropic.com",
                // Scheduled execution resolves persisted credentials through
                // native secure storage, never a scalar Keychain override.
                apiKey: "",
                model: task?.configuration.model ?? snapshot.defaultModelID ?? Keychain.get(.model) ?? "",
                sessionMode: .code,
                visionDelegationEnabled: snapshot.visionDelegationEnabled,
                appSandboxRoot: appSandboxRoot,
                projectCwd: scope.projectCwd ?? (task == nil ? nil : directories.scheduledCwd),
                projectAdditionalDirectories: [],
                projectMountGrants: [],
                providerConfig: IosProviderConfigFfi(
                    providerProfilesJson: snapshot.providerProfilesJSON, routingJson: snapshot.routingJSON
                ),
                mobileLinux: runtime.map { mobileLinuxConfiguration($0, appSandboxRoot: appSandboxRoot) },
                hostEnvironment: makeIosHostEnvironment(launchMode: .scheduledHeadless)
            )
            policy = .scheduled

        case let .appAgent(appSandboxRoot, snapshot):
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
                appSandboxRoot: appSandboxRoot
            )
            #else
            // Store App Agents use a provider-only host and do not prepare
            // Linux workspace/rootfs resources or register Linux-only tools.
            mobileLinux = nil
            #endif
            settings = LaunchSettings(
                appAgentHost: true,
                apiBase: Keychain.get(.apiBase) ?? "https://api.anthropic.com",
                apiKey: Keychain.get(.apiKey) ?? "",
                model: snapshot.defaultModelID ?? "",
                sessionMode: .code,
                visionDelegationEnabled: false,
                appSandboxRoot: appSandboxRoot,
                projectCwd: nil,
                projectAdditionalDirectories: [],
                projectMountGrants: [],
                providerConfig: IosProviderConfigFfi(
                    providerProfilesJson: snapshot.providerProfilesJSON, routingJson: snapshot.routingJSON
                ),
                mobileLinux: mobileLinux,
                hostEnvironment: nil
            )
            policy = .appAgent
        }

        return Prepared(
            configuration: IosEngineLaunchConfigFfi(
                appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String,
                appAgentHost: settings.appAgentHost,
                brandId: directories.brandId,
                linuxUser: directories.linuxUser,
                configDirName: directories.configDirName,
                nativeConfigHome: directories.configHome,
                apiBase: settings.apiBase,
                apiKey: settings.apiKey,
                model: settings.model,
                sessionMode: settings.sessionMode,
                visionDelegationEnabled: settings.visionDelegationEnabled,
                appSandboxRoot: settings.appSandboxRoot,
                projectCwd: settings.projectCwd,
                projectAdditionalDirectories: settings.projectAdditionalDirectories,
                projectMountGrants: settings.projectMountGrants,
                providerConfig: settings.providerConfig,
                mobileLinux: settings.mobileLinux,
                physicalMemoryBytes: ProcessInfo.processInfo.physicalMemory,
                hostEnvironment: settings.hostEnvironment
            ),
            policy: policy
        )
    }

    /// The synchronous FFI constructor blocks on MCP initialization and can
    /// enter iSH. Use a dedicated large-stack thread for every scenario; a
    /// suspended task does not occupy the cooperative executor while it runs.
    static func build(
        _ prepared: Prepared,
        mediaHost: IosNativeMediaHost?,
        interactiveBuilder: HandleBuilder? = nil
    ) async throws -> MobileEngineHandle {
        let listener: IosEventListener
        let permissions: IosPermissionSink
        let headlessPermissions: NativeHeadlessPermissionSink?
        let provideLocation: Bool
        let migrateCredential: Bool
        let threadName: String
        switch prepared.policy {
        case let .interactive(interactiveListener, interactivePermissions):
            listener = interactiveListener
            permissions = interactivePermissions
            headlessPermissions = nil
            provideLocation = true
            migrateCredential = true
            threadName = "LingXi engine bootstrap"
        case .scheduled:
            let sink = NativeHeadlessPermissionSink()
            listener = HeadlessListener()
            permissions = sink
            headlessPermissions = sink
            provideLocation = false
            migrateCredential = true
            threadName = "LingXi scheduled engine bootstrap"
        case .appAgent:
            let sink = NativeHeadlessPermissionSink()
            listener = HeadlessListener()
            permissions = sink
            headlessPermissions = sink
            provideLocation = false
            migrateCredential = false
            threadName = "Local App model host"
        }

        try Task.checkCancellation()
        let handle: MobileEngineHandle = try await withCheckedThrowingContinuation { continuation in
            let thread = Thread {
                autoreleasepool {
                    continuation.resume(with: Result {
                        if let interactiveBuilder, case .interactive = prepared.policy {
                            return try interactiveBuilder(prepared.configuration, listener, mediaHost, permissions)
                        }
                        guard let mediaHost else {
                            throw NSError(
                                domain: "IOSNativeMediaHost", code: 1,
                                userInfo: [NSLocalizedDescriptionKey: "The app media host was not composed before engine creation."]
                            )
                        }
                        let location: IosLocation? = provideLocation ? LocationImpl() : nil
                        return try buildIosEngineWithConfig(
                            config: prepared.configuration,
                            listener: listener,
                            mediaHost: mediaHost,
                            share: ShareImpl(),
                            notifications: NotificationImpl(),
                            clipboard: ClipboardImpl(),
                            permissions: permissions,
                            secureStorage: SecureStorageImpl(),
                            deviceControl: DeviceControlImpl(),
                            location: location
                        )
                    })
                }
            }
            thread.name = threadName
            thread.qualityOfService = .userInitiated
            thread.stackSize = 8 * 1024 * 1024
            thread.start()
        }
        try Task.checkCancellation()
        if migrateCredential {
            try await migrateLegacyAnthropicCredential(on: handle)
        }
        headlessPermissions?.attach(handle)
        return handle
    }

    /// Use the same sandbox root in the launch and runtime configurations so
    /// iSH validates mounts against the directory owned by this engine.
    static func mobileLinuxConfiguration(
        _ config: TerminalRuntimeConfig,
        appSandboxRoot: String
    ) -> IosMobileLinuxConfigFfi {
        LXISHRuntimeResourceBootstrap.prepare()
        let directories = MobileUserStorage.directories()
        return IosMobileLinuxConfigFfi(
            mode: config.mode == .legacy ? .legacy : .mobileLinux,
            brandId: directories.brandId,
            linuxUser: directories.linuxUser,
            configDirName: directories.configDirName,
            managedRoot: config.managedRoot,
            workspaceHostPath: config.workspaceHostPath,
            stableWorkspaceId: config.stableWorkspaceId,
            abi: config.abi,
            rootfsVersion: config.rootfsVersion,
            archiveSha256: config.archiveSha256,
            authorizationFile: config.authorizationFile,
            appSandboxRoot: appSandboxRoot
        )
    }
}

private final class HeadlessListener: IosEventListener, @unchecked Sendable {
    func onEvent(event: ClientEvent) async {}
    func onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) async {}
}

/// The engine retains this callback, so it must not retain the engine back.
final class NativeHeadlessPermissionSink: IosPermissionSink, @unchecked Sendable {
    private let lock = NSLock()
    private weak var handle: MobileEngineHandle?

    func attach(_ handle: MobileEngineHandle) { lock.withLock { self.handle = handle } }

    func onRequest(request: PermissionRequest) async {
        let current = lock.withLock { handle }
        try? await current?.submit(command: .denyPermission(requestId: request.requestId))
    }
}

#endif
