import XCTest
@testable import LingxiCode

@MainActor
final class TerminalSessionModelTests: XCTestCase {
    func testStartIfNeededRejectsWrongWorkspaceInsteadOfFallingBack() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/other"],
                lastError: nil
            )
        )
        let model = TerminalSessionModel(
            descriptor: TerminalRuntimeDescriptor(
                config: nil,
                workspace: TerminalWorkspaceDescriptor(hostPath: nil, guestPath: "/workspace/project", displayName: "Project")
            ),
            client: client
        )

        await model.startIfNeeded()

        XCTAssertEqual(model.availability, .workspaceUnavailable("工作目录未挂载到 guest，已拒绝回退到错误目录"))
        XCTAssertNil(model.activeSessionID)
        let openRequestCount = await client.recordedOpenRequestCount()
        XCTAssertEqual(openRequestCount, 0)
    }

    func testLifecycleConsumesEventsAndMarksExit() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/project"],
                lastError: nil
            ),
            tasks: [TerminalTaskSnapshot(id: "t1", title: "shell", state: .running, detail: nil)],
            openSession: TerminalPtySession(id: "pty-1", available: true, detail: nil),
            eventBatches: [[
                TerminalStreamEvent(
                    sequence: 1,
                    taskId: "t1",
                    streamId: "pty-1",
                    source: .pty,
                    kind: .stdoutLine,
                    text: "hello\n",
                    data: nil,
                    exitCode: nil,
                    timedOut: false
                ),
                TerminalStreamEvent(
                    sequence: 2,
                    taskId: "t1",
                    streamId: "pty-1",
                    source: .pty,
                    kind: .exit,
                    text: nil,
                    data: nil,
                    exitCode: 0,
                    timedOut: false
                ),
            ]]
        )
        let model = TerminalSessionModel(
            descriptor: TerminalRuntimeDescriptor(
                config: nil,
                workspace: TerminalWorkspaceDescriptor(hostPath: nil, guestPath: "/workspace/project", displayName: "Project"),
                pollInterval: .milliseconds(1),
                taskRefreshInterval: 1
            ),
            client: client
        )

        await model.startIfNeeded()
        try? await Task.sleep(for: .milliseconds(30))

        XCTAssertEqual(model.buffer.plainText, "hello\n")
        XCTAssertEqual(model.lastExitStatus, TerminalExitStatus(code: 0, timedOut: false))
        XCTAssertEqual(model.availability, .closed)
        XCTAssertFalse(model.isPolling)

        let readsAfterExit = await client.recordedEventReadCursors().count
        try? await Task.sleep(for: .milliseconds(10))
        let readsAfterSettling = await client.recordedEventReadCursors().count
        XCTAssertEqual(readsAfterSettling, readsAfterExit)
    }

    func testPollingAdvancesGlobalCursorPastUnrelatedStreams() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/project"],
                lastError: nil
            ),
            openSession: TerminalPtySession(id: "pty-current", available: true, detail: nil),
            eventBatches: [
                [
                    TerminalStreamEvent(
                        sequence: 41,
                        taskId: nil,
                        streamId: "runtime",
                        source: .run,
                        kind: .error,
                        text: "unrelated runtime event",
                        data: nil,
                        exitCode: nil,
                        timedOut: false
                    )
                ],
                [
                    TerminalStreamEvent(
                        sequence: 42,
                        taskId: nil,
                        streamId: "pty-current",
                        source: .pty,
                        kind: .stdoutLine,
                        text: "ready\n",
                        data: nil,
                        exitCode: nil,
                        timedOut: false
                    ),
                    TerminalStreamEvent(
                        sequence: 43,
                        taskId: nil,
                        streamId: "pty-current",
                        source: .pty,
                        kind: .exit,
                        text: nil,
                        data: nil,
                        exitCode: 0,
                        timedOut: false
                    ),
                ],
            ]
        )
        let model = TerminalSessionModel(
            descriptor: TerminalRuntimeDescriptor(
                config: nil,
                workspace: TerminalWorkspaceDescriptor(
                    hostPath: nil,
                    guestPath: "/workspace/project",
                    displayName: "Project"
                ),
                pollInterval: .milliseconds(1)
            ),
            client: client
        )

        await model.startIfNeeded()
        try? await Task.sleep(for: .milliseconds(30))

        XCTAssertEqual(model.buffer.plainText, "ready\n")
        XCTAssertEqual(model.lastSequence, 43)
        let cursors = await client.recordedEventReadCursors()
        XCTAssertGreaterThanOrEqual(cursors.count, 2)
        XCTAssertNil(cursors[0])
        XCTAssertEqual(cursors[1], 41)
    }

    func testLifecyclePrefersBinaryPayloadWhenTextAndDataBothPresent() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/project"],
                lastError: nil
            ),
            openSession: TerminalPtySession(id: "pty-dup", available: true, detail: nil),
            eventBatches: [[
                TerminalStreamEvent(
                    sequence: 1,
                    taskId: nil,
                    streamId: "pty-dup",
                    source: .pty,
                    kind: .stdoutLine,
                    text: "dup",
                    data: Data("dup".utf8),
                    exitCode: nil,
                    timedOut: false
                )
            ]]
        )
        let model = TerminalSessionModel(
            descriptor: TerminalRuntimeDescriptor(
                config: nil,
                workspace: TerminalWorkspaceDescriptor(hostPath: nil, guestPath: "/workspace/project", displayName: "Project"),
                pollInterval: .milliseconds(1)
            ),
            client: client
        )

        await model.startIfNeeded()
        try? await Task.sleep(for: .milliseconds(20))

        XCTAssertEqual(model.buffer.plainText, "dup")
    }

    func testInvalidRequestedCwdIsSurfacedBeforeOpen() async {
        let client = FakeTerminalRuntimeClient(
            capability: TerminalCapabilitySnapshot(
                available: true,
                backend: "ios-ish",
                mode: .mobileLinux,
                reason: nil,
                streamingOutput: true,
                backgroundProcesses: true,
                pty: true,
                bindMounts: true,
                rootfsIntegrity: true
            ),
            status: TerminalStatusSnapshot(
                state: .ready,
                backend: "ios-ish",
                mode: .mobileLinux,
                platform: "ios",
                abi: "arm64",
                version: "1.0.0",
                managedRoot: nil,
                activeRoot: nil,
                stagedRoot: nil,
                archiveSha256: nil,
                installedSizeBytes: nil,
                writableGuestPaths: ["/workspace/project"],
                lastError: nil
            )
        )
        let descriptor = TerminalRuntimeDescriptor(
            config: nil,
            workspace: TerminalWorkspaceDescriptor(hostPath: nil, guestPath: "/workspace/project", displayName: "Project"),
            launchCwd: nil,
            invalidRequestedCwdMessage: "请求的 cwd 超出 workspace：/tmp"
        )
        let model = TerminalSessionModel(descriptor: descriptor, client: client)

        await model.startIfNeeded()

        XCTAssertEqual(model.availability, .invalidRequest("请求的 cwd 超出 workspace：/tmp"))
        let openRequestCount = await client.recordedOpenRequestCount()
        XCTAssertEqual(openRequestCount, 0)
    }

    // MARK: - Recovery mapping

    /// Two causes may share a button, but never a headline: reading
    /// "Linux 运行时不可用" when the runtime is fine and only the mount failed
    /// sends the reader looking in the wrong place.
    func testEachUnavailableCauseMapsToItsOwnRemedy() {
        XCTAssertEqual(TerminalRecovery.forState(.workspaceUnavailable("x")), .workspaceNotMounted)
        XCTAssertEqual(TerminalRecovery.forState(.unavailable("x")), .runtimeUnavailable)
        XCTAssertEqual(TerminalRecovery.forState(.integrityFailure("x")), .repairRuntime)

        // Distinctness is the property that broke; assert it directly rather
        // than trusting three equality checks to stay different.
        let titles = Set([
            TerminalRecovery.forState(.workspaceUnavailable("x")).title,
            TerminalRecovery.forState(.unavailable("x")).title,
            TerminalRecovery.forState(.integrityFailure("x")).title,
        ])
        XCTAssertEqual(titles.count, 3, "each cause needs a distinct headline")
    }

    /// A shell is not a property of a project. With no project open the
    /// terminal must still start, in the runtime's own persistent home.
    func testTerminalOpensWithoutAProject() {
        let descriptor = TerminalRuntimeDescriptor.make(
            appSandboxRoot: NSTemporaryDirectory(),
            project: nil,
            linuxRuntime: LinuxRuntimeState()
        )
        XCTAssertNotNil(
            descriptor.config,
            "a project-less terminal still needs a runtime config, or no shell can start"
        )
        XCTAssertEqual(descriptor.workspace.guestPath, LXISHDefaultWorkspace.guestHome)
        XCTAssertEqual(descriptor.launchCwd, LXISHDefaultWorkspace.guestHome)
        XCTAssertNil(descriptor.invalidRequestedCwdMessage)
    }

    /// The fallback workspace must be stable — a fresh UUID per launch would
    /// mint a throwaway workspace every time the terminal opened.
    func testProjectlessWorkspaceIsStableAcrossCalls() {
        let a = TerminalRuntimeDescriptor.make(
            appSandboxRoot: NSTemporaryDirectory(), project: nil, linuxRuntime: LinuxRuntimeState()
        )
        let b = TerminalRuntimeDescriptor.make(
            appSandboxRoot: NSTemporaryDirectory(), project: nil, linuxRuntime: LinuxRuntimeState()
        )
        XCTAssertEqual(a.config?.stableWorkspaceId, b.config?.stableWorkspaceId)
        XCTAssertFalse(a.config?.stableWorkspaceId.isEmpty ?? true)
    }

    func testHealthyAndUnrecoverableStatesOfferNoRepair() {
        let states: [TerminalAvailabilityState] = [
            .idle, .opening, .ready, .closed,
            .invalidRequest("bad cwd"), .failed("boom"),
        ]
        for state in states {
            XCTAssertEqual(
                TerminalRecovery.forState(state), TerminalRecovery.none,
                "\(state) must not offer a repair button"
            )
        }
    }
}
