import Foundation
import XCTest

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
@MainActor
final class NativeEngineFactoryTests: XCTestCase {
    /// Run alone in a fresh simulator test process before the other native
    /// tests, so callback registration and the object's free path cannot rely
    /// on a prior runtime call made by this test.
    func testFreshProcessCronAdmissionConstructsAndReleases() {
        weak var released: MobileCronAdmission?
        autoreleasepool {
            let admission = MobileCronAdmission(owner: RejectingCronOwnerAdmission())
            released = admission
            withExtendedLifetime(admission) {}
        }
        XCTAssertNil(released)
    }

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
        config.initialSessionId = "8b625566-3a09-4d32-88f5-785e2ca97b2f"
        config.initialSessionTranscriptPath = "/home/lingxi/.lingxi/projects/original/session.jsonl"
        let launch = try NativeEngineFactory.prepare(.interactive(
            config: config, listener: FactoryTestListener(), permissions: FactoryTestPermissions()
        )).configuration

        XCTAssertFalse(launch.appAgentHost)
        XCTAssertFalse(launch.schedulerController)
        XCTAssertEqual(launch.sessionMode, .chat)
        XCTAssertEqual(launch.initialSessionId, config.initialSessionId)
        XCTAssertEqual(launch.initialSessionTranscriptPath, config.initialSessionTranscriptPath)
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
        automation.sessionMode = .code
        automation.sessionScopeKey = ConversationScope.scheduled.workspaceKey
        let task = CronTaskRecord(
            id: "task", cron: "0 * * * *", prompt: "test", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "test",
            automation: automation
        )
        let launch = try NativeEngineFactory.prepare(.scheduled(
            appSandboxRoot: "/sandbox", scope: global, task: task, snapshot: snapshot, runtime: nil
        )).configuration

        XCTAssertFalse(launch.appAgentHost)
        XCTAssertTrue(launch.schedulerController)
        XCTAssertNil(launch.initialSessionId)
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

    func testColdRestoreRequiresExactTranscriptPathBeforeNativeBuild() throws {
        var config = EngineConfig(apiBase: "https://example.invalid", model: "",
            appSandboxRoot: "/sandbox", projectCwd: "/workspace", sessionMode: .chat,
            visionDelegationEnabled: false, initialSessionId: UUID().uuidString)
        XCTAssertThrowsError(try NativeEngineFactory.prepare(.interactive(
            config: config, listener: FactoryTestListener(), permissions: FactoryTestPermissions())))
        config.initialSessionTranscriptPath = "/home/lingxi/.lingxi/projects/exact/session.jsonl"
        let prepared = try NativeEngineFactory.prepare(.interactive(
            config: config, listener: FactoryTestListener(), permissions: FactoryTestPermissions()))
        XCTAssertEqual(prepared.configuration.initialSessionTranscriptPath, config.initialSessionTranscriptPath)
        config.initialSessionId = nil
        XCTAssertThrowsError(try NativeEngineFactory.prepare(.interactive(
            config: config, listener: FactoryTestListener(), permissions: FactoryTestPermissions())))
    }

    func testNativeEmptyOwnerColdRestoresOnlyItsOriginalPathAndMode() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("empty-owner-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let dirs = MobileUserStorage.directories(managedRoot: root.appendingPathComponent("runtime").path)
        let mediaHost = try IOSNativeMediaHostComposition(
            audioConfigurationJSON: try XCTUnwrap(AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON)).nativeHost
        func build(mode: SessionModeDto = .chat, sessionID: String? = nil, path: String? = nil) throws -> MobileEngineHandle {
            let launch = try mobileWorkspaceTestLaunchConfiguration(sandboxRoot: root, apiBase: "http://127.0.0.1:9", model: "",
                sessionMode: mode, initialSessionId: sessionID, initialSessionTranscriptPath: path)
            return try buildIosEngineWithConfig(config: launch, listener: NoopEventListener(), mediaHost: mediaHost,
                share: ShareImpl(), notifications: NotificationImpl(), clipboard: ClipboardImpl(),
                permissions: NoopPermissionSink(), secureStorage: nil, deviceControl: nil)
        }
        var first: MobileEngineHandle? = try build()
        let owner = await first!.workspaceState()
        XCTAssertNotNil(UUID(uuidString: owner.sessionId))
        XCTAssertTrue((owner.transcriptPath as NSString).isAbsolutePath)
        first = nil
        let firstReleased = await waitForMobileSessionRelease(lingxiHome: dirs.configHome, sessionId: owner.sessionId, timeoutMs: 8_000)
        XCTAssertTrue(firstReleased)
        guard firstReleased else { return }

        var restored: MobileEngineHandle? = try build(sessionID: owner.sessionId, path: owner.transcriptPath)
        let restoredOwner = await restored!.workspaceState()
        XCTAssertEqual(restoredOwner.sessionId, owner.sessionId)
        XCTAssertEqual(restoredOwner.transcriptPath, owner.transcriptPath)
        restored = nil
        let restoredReleased = await waitForMobileSessionRelease(lingxiHome: dirs.configHome, sessionId: owner.sessionId, timeoutMs: 8_000)
        XCTAssertTrue(restoredReleased)
        guard restoredReleased else { return }

        XCTAssertThrowsError(try build(mode: .code, sessionID: owner.sessionId, path: owner.transcriptPath),
                             "A fixed Chat owner cannot reopen as Code")
        let guestPrefix = dirs.guestHome + "/"
        let hostSource = owner.transcriptPath.hasPrefix(guestPrefix)
            ? URL(fileURLWithPath: dirs.home).appendingPathComponent(String(owner.transcriptPath.dropFirst(guestPrefix.count)))
            : URL(fileURLWithPath: owner.transcriptPath)
        let contained = hostSource.resolvingSymlinksInPath().standardizedFileURL.path
            .hasPrefix(root.resolvingSymlinksInPath().standardizedFileURL.path + "/")
        XCTAssertTrue(contained, "Only the isolated test root may be changed")
        guard contained else { return }
        try FileManager.default.removeItem(at: hostSource)
        XCTAssertThrowsError(try build(sessionID: owner.sessionId, path: owner.transcriptPath),
                             "A missing exact source must never fall back to a UUID search")
    }

    func testNativeCdColdRestoresCwdFromTheOriginalExactSource() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("cd-owner-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let dirs = MobileUserStorage.directories(managedRoot: root.appendingPathComponent("runtime").path)
        let movedDirectory = URL(fileURLWithPath: dirs.home).appendingPathComponent("moved", isDirectory: true)
        try FileManager.default.createDirectory(at: movedDirectory, withIntermediateDirectories: true)
        let targetCwd = dirs.guestHome + "/moved"
        let mediaHost = try IOSNativeMediaHostComposition(
            audioConfigurationJSON: try XCTUnwrap(AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON)).nativeHost
        func build(sessionID: String? = nil, path: String? = nil) throws -> MobileEngineHandle {
            let launch = try mobileWorkspaceTestLaunchConfiguration(sandboxRoot: root, apiBase: "http://127.0.0.1:9", model: "",
                sessionMode: .chat, initialSessionId: sessionID, initialSessionTranscriptPath: path)
            return try buildIosEngineWithConfig(config: launch, listener: NoopEventListener(), mediaHost: mediaHost,
                share: ShareImpl(), notifications: NotificationImpl(), clipboard: ClipboardImpl(),
                permissions: NoopPermissionSink(), secureStorage: nil, deviceControl: nil)
        }
        var first: MobileEngineHandle? = try build()
        let original = await first!.workspaceState()
        try await first!.changeWorkspace(path: targetCwd, expectedSessionId: original.sessionId)
        let moved = await first!.workspaceState()
        XCTAssertEqual(moved.cwd, targetCwd)
        XCTAssertEqual(moved.transcriptPath, original.transcriptPath)
        first = nil
        let released = await waitForMobileSessionRelease(lingxiHome: dirs.configHome, sessionId: original.sessionId, timeoutMs: 8_000)
        XCTAssertTrue(released)
        guard released else { return }

        let restored = try build(sessionID: original.sessionId, path: original.transcriptPath)
        let state = await restored.workspaceState()
        XCTAssertEqual(state.sessionId, original.sessionId)
        XCTAssertEqual(state.transcriptPath, original.transcriptPath)
        XCTAssertEqual(state.cwd, targetCwd)
        withExtendedLifetime(restored) {}
    }

    func testSelectedCronUsesCapturedChatMode() throws {
        var automation = CronAutomation()
        automation.model = "task-model"
        automation.runMode = .selectedSession
        automation.targetSessionId = UUID().uuidString
        automation.targetSessionMode = .chat
        automation.targetSessionTranscriptPath = "/home/lingxi/source.jsonl"
        automation.targetSessionScopeKey = ConversationScope.global.workspaceKey
        let task = CronTaskRecord(id: "task", cron: "0 * * * *", prompt: "test", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "test", automation: automation)
        let launch = try NativeEngineFactory.prepare(.scheduled(appSandboxRoot: "/sandbox",
            scope: .global(appSandboxRoot: "/sandbox"), task: task, snapshot: makeSnapshot(), runtime: nil)).configuration
        XCTAssertEqual(launch.sessionMode, .chat)
    }

    func testNativeScheduledControllerDoesNotCreateAConversationSource() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("scheduler-controller-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try await exerciseIsolatedSchedulerController(root: root)
    }

    private func exerciseIsolatedSchedulerController(root: URL) async throws {
        let config = try mobileWorkspaceTestLaunchConfiguration(sandboxRoot: root, apiBase: "http://127.0.0.1:9", model: "",
                                                                schedulerController: true)
        let mediaHost = try IOSNativeMediaHostComposition(
            audioConfigurationJSON: try XCTUnwrap(AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON)
        ).nativeHost
        let handle = try buildIosEngineWithConfig(config: config, listener: NoopEventListener(), mediaHost: mediaHost,
            share: ShareImpl(), notifications: NotificationImpl(), clipboard: ClipboardImpl(),
            permissions: NoopPermissionSink(), secureStorage: nil, deviceControl: nil)
        try await Task.sleep(for: .milliseconds(100))
        let files = FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil)?.allObjects.compactMap { $0 as? URL } ?? []
        XCTAssertTrue(files.filter { $0.pathExtension == "jsonl" }.isEmpty,
                      "A scheduler controller must not create an unreserved transcript")
        withExtendedLifetime(handle) {}
    }

    func testNativeSchedulerCallsSwiftAdmissionBeforeAnyTranscriptWrite() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("scheduler-admission-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try await exerciseNativeSchedulerAdmission(root: root)
    }

    private func exerciseNativeSchedulerAdmission(root: URL) async throws {
        let (handle, store, task) = try await makeNativeSchedulerFixture(root: root)
        let gate = CronOwnerAdmissionGate()
        var beginCount = 0
        let callback = FfiCronOwnerAdmission(gate: gate, scope: .global(appSandboxRoot: root.path), task: task,
            prepare: { candidate, _, _ in
                beginCount += 1
                XCTAssertNil(candidate.sessionTranscriptPath)
                XCTAssertEqual(candidate.sessionMode, .chat)
                XCTAssertEqual(candidate.sessionScopeKey, "scheduled")
                return false
            }, confirm: { _, _, _ in XCTFail("Rejected owner must not be confirmed") },
            blockAdmission: { _ in XCTFail("A busy owner is not catalog corruption") })
        let admission = MobileCronAdmission(owner: callback)
        let result = await handle.runCronTaskNow(taskId: task.id, ownerAdmission: admission)
        await admission.cancelAndWait()
        XCTAssertEqual(beginCount, 1)
        XCTAssertTrue(try XCTUnwrap(result).retryable)
        let reservations = try await store.listOwnerReservationsStrict()
        XCTAssertTrue(reservations.isEmpty)
        let files = FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil)?.allObjects.compactMap { $0 as? URL } ?? []
        XCTAssertTrue(files.filter { $0.pathExtension == "jsonl" }.isEmpty)
        withExtendedLifetime(handle) {}
    }

    func testNativeSchedulerCancellationWaitsForTheSwiftOwnerCallback() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("scheduler-cancel-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        try await exerciseNativeSchedulerCancellation(root: root)
    }

    func testNativeProjectSchedulerListFireAndLedgerKeepTheirRootAfterCd() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("scheduler-project-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let managedRoot = root.appendingPathComponent("runtime").path
        let dirs = MobileUserStorage.directories(managedRoot: managedRoot)
        let repository = ProjectRepository(projectsRoot: URL(fileURLWithPath: dirs.projects),
                                           userHome: URL(fileURLWithPath: dirs.home))
        var project = try repository.createInternal(name: "Project")
        let before = CronScope.project(project)
        let moved = URL(fileURLWithPath: dirs.home).appendingPathComponent("elsewhere", isDirectory: true)
        try FileManager.default.createDirectory(at: moved, withIntermediateDirectories: true)
        project.record.activeDirectory = "/home/lingxi/elsewhere"
        project.workspace = ProjectWorkspace(projectId: project.id, hostURL: moved, guestPath: project.record.activeDirectory)
        let after = CronScope.project(project)
        XCTAssertEqual(before, after)

        let firstStore = try buildIosCronStore(appSandboxRoot: root.path, managedRoot: managedRoot, projectCwd: before.projectCwd)
        var automation = CronAutomation()
        automation.model = "provider/model"
        automation.sessionMode = .chat
        automation.sessionScopeKey = ConversationScope.project(project.id).workspaceKey
        let task = try await firstStore.createConfigured(cronExpr: "0 9 * * *", prompt: "Test", recurring: true,
            automationJson: String(decoding: JSONEncoder().encode(automation), as: UTF8.self))
        let secondStore = try buildIosCronStore(appSandboxRoot: root.path, managedRoot: managedRoot, projectCwd: after.projectCwd)
        let tasks = try await secondStore.listStrict()
        XCTAssertEqual(tasks.map(\.id), [task.id])

        let launch = try mobileWorkspaceTestLaunchConfiguration(sandboxRoot: root, apiBase: "http://127.0.0.1:9", model: "",
                                                                schedulerController: true, projectCwd: after.projectCwd)
        let mediaHost = try IOSNativeMediaHostComposition(
            audioConfigurationJSON: try XCTUnwrap(AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON)).nativeHost
        let handle = try buildIosEngineWithConfig(config: launch, listener: NoopEventListener(), mediaHost: mediaHost,
            share: ShareImpl(), notifications: NotificationImpl(), clipboard: ClipboardImpl(),
            permissions: NoopPermissionSink(), secureStorage: nil, deviceControl: nil)
        let callback = BeginOnlyCronOwnerAdmission()
        let admission = MobileCronAdmission(owner: callback)
        _ = await handle.runCronTaskNow(taskId: task.id, ownerAdmission: admission)
        await admission.cancelAndWait()
        let beginCount = await callback.beginCount
        XCTAssertEqual(beginCount, 1)
        let originalReservations = try await firstStore.listOwnerReservationsStrict()
        let movedReservations = try await secondStore.listOwnerReservationsStrict()
        XCTAssertEqual(originalReservations.count, 1)
        XCTAssertEqual(movedReservations.map(\.sessionId), originalReservations.map(\.sessionId))
        XCTAssertEqual(movedReservations.first?.sessionTranscriptPath, originalReservations.first?.sessionTranscriptPath)
        XCTAssertEqual(movedReservations.first?.sessionScopeKey, ConversationScope.project(project.id).workspaceKey)
        withExtendedLifetime(handle) {}
    }

    private func exerciseNativeSchedulerCancellation(root: URL) async throws {
        let (handle, store, task) = try await makeNativeSchedulerFixture(root: root)
        let started = expectation(description: "Swift owner callback started")
        let callback = PausingCronOwnerAdmission(started: started)
        let admission = MobileCronAdmission(owner: callback)
        let fire = Task { await handle.runCronTaskNow(taskId: task.id, ownerAdmission: admission) }
        await fulfillment(of: [started], timeout: 10)
        guard await callback.didStart else {
            fire.cancel()
            await callback.release()
            await admission.cancelAndWait()
            return XCTFail("Native scheduler did not invoke owner admission")
        }
        fire.cancel()
        let finished = CronTestFlag()
        let barrier = Task { await admission.cancelAndWait(); await finished.set() }
        try await Task.sleep(for: .milliseconds(100))
        let finishedBeforeCallback = await finished.value
        XCTAssertFalse(finishedBeforeCallback, "Cancellation must wait for the in-flight Swift callback")
        await callback.release()
        await barrier.value
        _ = await fire.value
        let confirmations = await callback.confirmationCount
        XCTAssertEqual(confirmations, 0)
        let reservations = try await store.listOwnerReservationsStrict()
        XCTAssertTrue(reservations.isEmpty)
        let files = FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil)?.allObjects.compactMap { $0 as? URL } ?? []
        XCTAssertTrue(files.filter { $0.pathExtension == "jsonl" }.isEmpty)
        withExtendedLifetime(handle) {}
    }

    private func makeNativeSchedulerFixture(root: URL) async throws -> (MobileEngineHandle, MobileCronStoreHandle, CronTaskRecord) {
        let managedRoot = root.appendingPathComponent("runtime").path
        let directories = MobileUserStorage.directories(managedRoot: managedRoot)
        let store = try buildIosCronStore(appSandboxRoot: root.path, managedRoot: managedRoot, projectCwd: nil)
        var automation = CronAutomation()
        automation.model = "provider/model"
        automation.sessionMode = .chat
        automation.sessionScopeKey = "scheduled"
        let created = try await store.createConfigured(cronExpr: "0 9 * * *", prompt: "Test", recurring: true,
            automationJson: String(decoding: JSONEncoder().encode(automation), as: UTF8.self))
        let launch = try mobileWorkspaceTestLaunchConfiguration(sandboxRoot: root, apiBase: "http://127.0.0.1:9", model: "",
                                                                schedulerController: true, projectCwd: directories.scheduledCwd)
        let mediaHost = try IOSNativeMediaHostComposition(
            audioConfigurationJSON: try XCTUnwrap(AudioConfigurationStore.shared.mediaRuntimeConfigurationJSON)
        ).nativeHost
        let handle = try buildIosEngineWithConfig(config: launch, listener: NoopEventListener(), mediaHost: mediaHost,
            share: ShareImpl(), notifications: NotificationImpl(), clipboard: ClipboardImpl(),
            permissions: NoopPermissionSink(), secureStorage: nil, deviceControl: nil)
        let task = CronTaskRecord(id: created.id, cron: "0 9 * * *", prompt: "Test", createdAtMs: 0,
            lastFiredAtMs: nil, recurring: true, nextFireMs: nil, human: "Daily", automation: automation)
        return (handle, store, task)
    }

    func testStoreAppAgentUsesStructuredProviderOnlyConfiguration() throws {
        guard !LocalAppAgentModelHost.usesMobileLinuxRuntime else {
            throw XCTSkip("The Full app prepares its managed Linux workspace")
        }
        let prepared = try NativeEngineFactory.prepare(.appAgent(
            appSandboxRoot: "/sandbox", snapshot: makeSnapshot()))
        let launch = prepared.configuration
        XCTAssertTrue(launch.appAgentHost)
        XCTAssertFalse(launch.schedulerController)
        XCTAssertEqual(launch.apiBase, "https://api.anthropic.com")
        XCTAssertEqual(launch.model, "default-model")
        XCTAssertFalse(launch.visionDelegationEnabled)
        XCTAssertNil(launch.projectCwd)
        XCTAssertTrue(launch.projectAdditionalDirectories.isEmpty)
        XCTAssertTrue(launch.projectMountGrants.isEmpty)
        XCTAssertNil(launch.mobileLinux)
        XCTAssertNil(launch.hostEnvironment)
        XCTAssertNoThrow(try prepared.validateMediaHost(nil))
        let changed = try NativeEngineFactory.prepare(.appAgent(
            appSandboxRoot: "/sandbox", snapshot: makeSnapshot(model: "another-model"))).configuration
        XCTAssertNotEqual(LocalAppAgentModelHostConfiguration.digest(launch), LocalAppAgentModelHostConfiguration.digest(changed))
    }

    func testInteractiveAndScheduledEnginesStillRequireMediaHost() throws {
        let config = EngineConfig(
            apiBase: "https://example.invalid", model: "",
            appSandboxRoot: "/sandbox", projectCwd: nil, sessionMode: .code,
            visionDelegationEnabled: false
        )
        let interactive = try NativeEngineFactory.prepare(.interactive(
            config: config, listener: FactoryTestListener(), permissions: FactoryTestPermissions()
        ))
        let scheduled = try NativeEngineFactory.prepare(.scheduled(
            appSandboxRoot: "/sandbox", scope: .global(appSandboxRoot: "/sandbox"),
            task: nil, snapshot: makeSnapshot(), runtime: nil
        ))
        for prepared in [interactive, scheduled] {
            XCTAssertThrowsError(try prepared.validateMediaHost(nil)) { error in
                XCTAssertEqual((error as NSError).domain, "IOSNativeMediaHost")
                XCTAssertEqual((error as NSError).code, 1)
            }
        }
    }

    func testCancelledBootstrapDoesNotPublishConstructedHandle() async throws {
        let config = EngineConfig(
            apiBase: "https://example.invalid", model: "",
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

    private func makeSnapshot(model: String = "default-model") -> ProviderLaunchSnapshot {
        ProviderLaunchSnapshot(
            profiles: [], providerProfilesJSON: "profiles", routingJSON: "routing",
            defaultModelID: model, enabledProfileIDs: [], visionDelegationEnabled: true
        )
    }
}

private final class FactoryTestListener: IosEventListener, @unchecked Sendable {
    func onEvent(event: ClientEvent) async {}
    func onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) async {}
}

private actor RejectingCronOwnerAdmission: MobileCronOwnerAdmission {
    func beginOwner(candidate: CronOwnerCandidateDto) async -> Bool { false }
    func confirmOwner(reservation: CronOwnerReservationDto) async -> Bool { false }
}

private actor BeginOnlyCronOwnerAdmission: MobileCronOwnerAdmission {
    private(set) var beginCount = 0
    func beginOwner(candidate: CronOwnerCandidateDto) async -> Bool { beginCount += 1; return true }
    func confirmOwner(reservation: CronOwnerReservationDto) async -> Bool { false }
}

private actor PausingCronOwnerAdmission: MobileCronOwnerAdmission {
    private let started: XCTestExpectation
    private var releaseContinuation: CheckedContinuation<Void, Never>?
    private var released = false
    private(set) var didStart = false
    private(set) var confirmationCount = 0

    init(started: XCTestExpectation) { self.started = started }

    func beginOwner(candidate: CronOwnerCandidateDto) async -> Bool {
        didStart = true
        started.fulfill()
        if !released { await withCheckedContinuation { releaseContinuation = $0 } }
        return true
    }

    func confirmOwner(reservation: CronOwnerReservationDto) async -> Bool {
        confirmationCount += 1
        return false
    }

    func release() { released = true; releaseContinuation?.resume(); releaseContinuation = nil }
}

private actor CronTestFlag {
    private(set) var value = false
    func set() { value = true }
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
