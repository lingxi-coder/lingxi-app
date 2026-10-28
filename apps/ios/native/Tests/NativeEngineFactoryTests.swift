import Foundation
import XCTest

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
@MainActor
final class NativeEngineFactoryTests: XCTestCase {
    func testInteractiveConfigurationPreservesProjectMountsAndSandbox() throws {
        let runtime = TerminalRuntimeConfig(
            mode: .mobileLinux,
            managedRoot: "/runtime",
            workspaceHostPath: "/workspace",
            stableWorkspaceId: "project",
            abi: "arm64",
            rootfsVersion: "test",
            archiveSha256: nil,
            authorizationFile: nil
        )
        var config = EngineConfig(
            apiBase: "https://example.invalid",
            apiKey: "development-override",
            model: "interactive-model",
            appSandboxRoot: "/sandbox",
            projectCwd: "/workspace",
            sessionMode: .chat,
            providerProfilesJson: "profiles",
            providerRoutingJson: "routing",
            visionDelegationEnabled: true,
            mobileLinux: runtime
        )
        config.additionalWorkspaceDirectories = ["/home/lingxi/trusted"]
        config.projectMountGrants = ["/home/lingxi/trusted/project"]
        let launch = try NativeEngineFactory.prepare(.interactive(
            config: config, listener: FactoryTestListener(), permissions: FactoryTestPermissions()
        )).configuration

        XCTAssertFalse(launch.appAgentHost)
        XCTAssertEqual(launch.apiKey, "development-override")
        XCTAssertEqual(launch.sessionMode, .chat)
        XCTAssertEqual(launch.projectCwd, "/workspace")
        XCTAssertEqual(launch.projectAdditionalDirectories, config.additionalWorkspaceDirectories)
        XCTAssertEqual(launch.projectMountGrants, config.projectMountGrants)
        XCTAssertEqual(launch.mobileLinux?.appSandboxRoot, launch.appSandboxRoot)
        XCTAssertEqual(launch.hostEnvironment?.launchMode, .interactive)
        XCTAssertEqual(launch.providerConfig?.providerProfilesJson, "profiles")
        XCTAssertEqual(launch.providerConfig?.routingJson, "routing")
    }

    func testScheduledConfigurationUsesHeadlessCredentialsAndSelectedWorkspace() throws {
        let snapshot = makeSnapshot()
        let global = CronScope.global(appSandboxRoot: "/sandbox")
        var automation = CronAutomation()
        automation.model = "task-model"
        let task = CronTaskRecord(
            id: "task", cron: "0 * * * *", prompt: "test", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "test",
            automation: automation
        )
        let launch = try NativeEngineFactory.prepare(.scheduled(
            appSandboxRoot: "/sandbox", scope: global, task: task, snapshot: snapshot, runtime: nil
        )).configuration

        XCTAssertFalse(launch.appAgentHost)
        XCTAssertEqual(launch.apiKey, "")
        XCTAssertEqual(launch.model, "task-model")
        XCTAssertEqual(launch.projectCwd, MobileUserStorage.directories().scheduledCwd)
        XCTAssertTrue(launch.projectAdditionalDirectories.isEmpty)
        XCTAssertTrue(launch.projectMountGrants.isEmpty)
        XCTAssertEqual(launch.hostEnvironment?.launchMode, .scheduledHeadless)
        XCTAssertTrue(launch.visionDelegationEnabled)

        let project = CronScope(
            scopeID: "project", projectID: "project", projectName: "Project",
            projectCwd: "/project", guestWorkspacePath: nil
        )
        let projectLaunch = try NativeEngineFactory.prepare(.scheduled(
            appSandboxRoot: "/sandbox", scope: project, task: task, snapshot: snapshot, runtime: nil
        )).configuration
        XCTAssertEqual(projectLaunch.projectCwd, "/project")
        let noTask = try NativeEngineFactory.prepare(.scheduled(
            appSandboxRoot: "/sandbox", scope: global, task: nil, snapshot: snapshot, runtime: nil
        )).configuration
        XCTAssertNil(noTask.projectCwd)
        XCTAssertEqual(noTask.model, "default-model")
    }

    func testStoreAppAgentUsesProviderOnlyConfiguration() throws {
        guard !LocalAppAgentModelHost.usesMobileLinuxRuntime else {
            throw XCTSkip("The Full app prepares its real managed Linux workspace")
        }
        let launch = try NativeEngineFactory.prepare(.appAgent(
            appSandboxRoot: "/sandbox", snapshot: makeSnapshot()
        )).configuration

        XCTAssertTrue(launch.appAgentHost)
        XCTAssertEqual(launch.model, "default-model")
        XCTAssertFalse(launch.visionDelegationEnabled)
        XCTAssertNil(launch.projectCwd)
        XCTAssertTrue(launch.projectAdditionalDirectories.isEmpty)
        XCTAssertTrue(launch.projectMountGrants.isEmpty)
        XCTAssertNil(launch.mobileLinux)
        XCTAssertNil(launch.hostEnvironment)
    }

    func testCancelledBootstrapDoesNotPublishConstructedHandle() async throws {
        let config = EngineConfig(
            apiBase: "https://example.invalid", apiKey: "", model: "",
            appSandboxRoot: "/sandbox", projectCwd: nil, sessionMode: .code,
            visionDelegationEnabled: false
        )
        let prepared = try NativeEngineFactory.prepare(.interactive(
            config: config, listener: FactoryTestListener(), permissions: FactoryTestPermissions()
        ))
        let started = expectation(description: "blocking constructor started")
        let release = DispatchSemaphore(value: 0)
        let task = Task.detached {
            try await NativeEngineFactory.build(prepared, mediaHost: nil) { _, _, _, _ in
                XCTAssertFalse(Thread.isMainThread)
                XCTAssertGreaterThanOrEqual(Thread.current.stackSize, 8 * 1024 * 1024)
                started.fulfill()
                release.wait()
                return FactoryTestHandle()
            }
        }
        await fulfillment(of: [started], timeout: 2)
        task.cancel()
        release.signal()
        do {
            _ = try await task.value
            XCTFail("a cancelled bootstrap must not publish its handle")
        } catch is CancellationError {
            // The native constructor cannot be interrupted; its result is discarded.
        }
    }

    func testHeadlessPermissionDeniesWithoutRetainingItsEngine() async {
        let sink = NativeHeadlessPermissionSink()
        var handle: FactoryTestHandle? = FactoryTestHandle()
        weak var weakHandle = handle
        sink.attach(handle!)
        await sink.onRequest(request: PermissionRequest(
            requestId: 201,
            kind: .toolUseConfirm(toolName: "Shell", toolInputJson: "{}", defaultAllow: false),
            worker: WorkerInfoDto(name: "background", color: "background", team: nil),
            owner: nil,
            suppressAlwaysAllowRule: false,
            autoModePrompt: nil
        ))
        XCTAssertEqual(handle?.deniedRequests(), [201])
        handle = nil
        XCTAssertNil(weakHandle, "the engine owns the sink, so the sink must not own the engine")
    }

    private func makeSnapshot() -> ProviderLaunchSnapshot {
        ProviderLaunchSnapshot(
            profiles: [], providerProfilesJSON: "profiles", routingJSON: "routing",
            defaultModelID: "default-model", enabledProfileIDs: [], visionDelegationEnabled: true
        )
    }
}

private final class FactoryTestListener: IosEventListener, @unchecked Sendable {
    func onEvent(event: ClientEvent) async {}
    func onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) async {}
}

private final class FactoryTestPermissions: IosPermissionSink, @unchecked Sendable {
    func onRequest(request: PermissionRequest) async {}
}

private final class FactoryTestHandle: MobileEngineHandle, @unchecked Sendable {
    private let requestLock = NSLock()
    private var deniedIDs: [UInt64] = []

    required init(unsafeFromHandle handle: UInt64) { super.init(unsafeFromHandle: handle) }
    init() { super.init(noHandle: .init()) }

    override func submit(command: ClientCommand) async throws {
        if case let .denyPermission(requestId) = command {
            requestLock.withLock { deniedIDs.append(requestId) }
        }
    }

    func deniedRequests() -> [UInt64] { requestLock.withLock { deniedIDs } }
}
#endif
