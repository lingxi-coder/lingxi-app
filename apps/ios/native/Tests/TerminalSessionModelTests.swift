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

    /// The stopped states offer restart FROM THE SAME decision table the other
    /// remedies use — and `canRestart` is derived from it, never a parallel
    /// switch (the fork this test's predecessor accidentally documented).
    func testStoppedStatesMapToRestartAndCanRestartDerivesFromIt() {
        XCTAssertEqual(TerminalRecovery.forState(.closed), .restartSession)
        XCTAssertEqual(TerminalRecovery.forState(.failed("boom")), .restartSession)
        // Restartable states render their own line bare — a "终端不可用"
        // headline above a working restart link is the bug, not a feature.
        XCTAssertNil(TerminalRecovery.forState(.failed("boom")).title)
        XCTAssertNil(TerminalRecovery.forState(.closed).title)
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

    /// Return is the only submit path now that the Send button is gone, and a
    /// bare Return is a real command — it accepts a `[Y/n]` default, pages
    /// `less`, and answers a `read`. Dropping it makes the shell look hung.
    func testBareReturnSendsANewlineInsteadOfBeingSwallowed() async {
        let client = Self.readyClient()
        let model = TerminalSessionModel(descriptor: Self.workspaceDescriptor(), client: client)
        await model.startIfNeeded()
        XCTAssertTrue(model.availability.canInteract, "fixture must reach .ready")

        model.inputText = ""
        await model.submitInput()

        let writes = await client.recordedWrites()
        XCTAssertEqual(writes, ["\n"])
        XCTAssertTrue(model.history.isEmpty, "an empty line must not enter history")

        // This fixture polls every 1 ms and never receives a terminal event, so
        // an unclosed session leaves a MainActor loop running for the rest of
        // the test process and contends with every test after it.
        await model.close()
    }

    /// `.closed` carries no message, and the rewrite removed every control that
    /// used to render the disabled state, so without a notice the screen is a
    /// black rectangle that silently stopped accepting input.
    func testClosedSessionReportsHowTheShellExited() async {
        let client = Self.readyClient(
            eventBatches: [[
                TerminalStreamEvent(
                    sequence: 1,
                    taskId: "t1",
                    streamId: "pty-1",
                    source: .pty,
                    kind: .exit,
                    text: nil,
                    data: nil,
                    exitCode: 130,
                    timedOut: false
                )
            ]]
        )
        let model = TerminalSessionModel(descriptor: Self.workspaceDescriptor(), client: client)
        XCTAssertNil(model.exitNotice, "an idle terminal has not exited")

        await model.startIfNeeded()
        let closed = await Self.waitUntil { model.availability == .closed }
        XCTAssertTrue(closed, "poller must deliver the exit event, got \(model.availability)")
        let notice = model.exitNotice
        XCTAssertNotNil(notice, "a closed shell must say so")
        XCTAssertTrue(notice?.contains("130") == true, "exit code must be visible, got \(notice ?? "nil")")
    }

    /// `activeSessionID` is assigned only after four awaits, so the guard on it
    /// cannot stop a second caller that arrives mid-open. The runtime allows one
    /// PTY per handle and refuses the second, which is how a terminal that
    /// looked fine ended up reporting "only one PTY session is supported".
    func testConcurrentStartsOpenExactlyOnePty() async {
        let client = Self.readyClient()
        let model = TerminalSessionModel(descriptor: Self.workspaceDescriptor(), client: client)

        async let first: Void = model.startIfNeeded()
        async let second: Void = model.startIfNeeded()
        async let third: Void = model.startIfNeeded()
        _ = await (first, second, third)

        let opens = await client.recordedOpenRequestCount()
        XCTAssertEqual(opens, 1, "a second concurrent start must not open a second PTY")
        XCTAssertEqual(model.availability, .ready)

        await model.close()
    }

    /// An `.error` event lands in `.failed` without clearing `activeSessionID`,
    /// and `.failed` offers no repair action of its own — so if restart does not
    /// reach it, one mid-session failure is a dead caret the user can only leave
    /// by popping the screen and losing the transcript.
    func testRestartRecoversFromAMidSessionFailureNotJustAcleanExit() async {
        let client = Self.readyClient(
            eventBatches: [[
                TerminalStreamEvent(
                    sequence: 1,
                    taskId: "t1",
                    streamId: "pty-1",
                    source: .pty,
                    kind: .error,
                    text: "runtime went away",
                    data: nil,
                    exitCode: nil,
                    timedOut: false
                )
            ]]
        )
        let model = TerminalSessionModel(descriptor: Self.workspaceDescriptor(), client: client)
        await model.startIfNeeded()
        let failed = await Self.waitUntil { model.availability == .failed("runtime went away") }
        XCTAssertTrue(failed, "poller must deliver the error event, got \(model.availability)")
        XCTAssertNotNil(model.activeSessionID, "an .error event leaves the handle open")
        XCTAssertTrue(model.canRestart, ".failed must offer a way back")

        await model.restart()

        XCTAssertEqual(model.availability, .ready)
        XCTAssertNil(model.lastError)
        // The fake enforces the bridge's one-PTY slot, so reaching `.ready`
        // above already proves the stale session was closed BEFORE the reopen
        // — assert the order explicitly so a reorder fails with words.
        let closed = await client.recordedClosedSessions()
        XCTAssertEqual(closed, ["pty-1"], "restart must close the stale session first")
        XCTAssertNotEqual(model.activeSessionID, "pty-1", "the reopened session must be a new one")

        await model.close()
    }

    /// Both taps pass `canRestart` if the state does not change until after
    /// the first await — the loser's `openPty` then fails against the one-PTY
    /// runtime and paints `.failed` over the live shell the winner opened.
    func testDoubleTappingRestartOpensExactlyOneNewPty() async {
        let client = Self.readyClient(
            eventBatches: [[Self.errorEvent(streamID: "pty-1", text: "boom")]],
            closePtyDelay: .milliseconds(100)
        )
        let model = TerminalSessionModel(descriptor: Self.workspaceDescriptor(), client: client)
        await model.startIfNeeded()
        let failed = await Self.waitUntil { model.canRestart }
        XCTAssertTrue(failed, "fixture must reach a restartable state")

        async let first: Void = model.restart()
        async let second: Void = model.restart()
        _ = await (first, second)

        XCTAssertEqual(model.availability, .ready)
        let opens = await client.recordedOpenRequestCount()
        XCTAssertEqual(opens, 2, "initial open plus ONE reopen — a second reopen means reentrancy")
        let closed = await client.recordedClosedSessions()
        XCTAssertEqual(closed, ["pty-1"], "the stale session must be closed exactly once")

        await model.close()
    }

    /// The user taps 重新启动 shell and immediately swipes back. `close()` runs
    /// while `restart()` is parked in `closePty`; if the resumed restart still
    /// reaches `startIfNeeded`, it wipes the teardown flag and opens a PTY
    /// into the dismissed view — occupying the runtime's only interactive
    /// slot until the app is relaunched.
    func testCloseDuringRestartDoesNotOpenIntoADeadView() async {
        let client = Self.readyClient(
            eventBatches: [[Self.errorEvent(streamID: "pty-1", text: "boom")]],
            closePtyDelay: .milliseconds(200)
        )
        let model = TerminalSessionModel(descriptor: Self.workspaceDescriptor(), client: client)
        await model.startIfNeeded()
        let failed = await Self.waitUntil { model.canRestart }
        XCTAssertTrue(failed, "fixture must reach a restartable state")

        let restart = Task { await model.restart() }
        // Let restart reach its 200ms `closePty` park before tearing down.
        try? await Task.sleep(for: .milliseconds(20))
        await model.close()
        await restart.value

        XCTAssertEqual(model.availability, .closed)
        let opens = await client.recordedOpenRequestCount()
        XCTAssertEqual(opens, 1, "a torn-down restart must not reopen a PTY")
        let leaked = await client.currentActivePtyID()
        XCTAssertNil(leaked, "no session may stay open after teardown")
    }

    /// A nil exit code is a lost pipeline, not a clean exit — the only
    /// spontaneous producer in the real stack is the runtime's reader-death
    /// path. "[process exited with code 0]" claimed success for it.
    func testExitNoticeReportsUnknownWhenTheCodeIsMissing() async {
        let client = Self.readyClient(
            eventBatches: [[
                TerminalStreamEvent(
                    sequence: 1,
                    taskId: "t1",
                    streamId: "pty-1",
                    source: .pty,
                    kind: .exit,
                    text: nil,
                    data: nil,
                    exitCode: nil,
                    timedOut: false
                )
            ]]
        )
        let model = TerminalSessionModel(descriptor: Self.workspaceDescriptor(), client: client)
        await model.startIfNeeded()
        let closed = await Self.waitUntil { model.availability == .closed }
        XCTAssertTrue(closed, "poller must deliver the exit event")

        XCTAssertEqual(model.exitNotice, String(localized: "terminal_exit_notice_unknown"))
        XCTAssertFalse(model.exitNotice?.contains("0") == true, "must not claim code 0")
    }

    /// `.failed` keeps the session handle on purpose, and writes still reach
    /// the shell — so the input path must stay usable there, or ^C (which
    /// rides above the keyboard) is unreachable while a runaway command runs.
    func testFailedSessionWithALiveHandleStillAcceptsInput() async {
        let client = Self.readyClient(
            eventBatches: [[Self.errorEvent(streamID: "pty-1", text: "poll hiccup")]]
        )
        let model = TerminalSessionModel(descriptor: Self.workspaceDescriptor(), client: client)
        await model.startIfNeeded()
        let failed = await Self.waitUntil { model.canRestart }
        XCTAssertTrue(failed, "fixture must reach .failed")

        XCTAssertTrue(model.canAcceptInput, ".failed with a live handle must accept input")
        model.inputText = "kill %1"
        await model.submitInput()
        let writes = await client.recordedWrites()
        XCTAssertEqual(writes, ["kill %1\n"], "the write must reach the still-open session")

        await model.close()
        XCTAssertFalse(model.canAcceptInput, "a closed session accepts nothing")
    }

    /// The dead shell stopped mid-escape; the restarted shell's first output
    /// must not be parsed as the old sequence's tail. (The buffer keeps the
    /// transcript across restarts by design — only the byte-stream state
    /// machines must reset.)
    func testRestartResetsTheAnsiParserState() async {
        let client = Self.readyClient(
            eventBatches: [
                [
                    TerminalStreamEvent(
                        sequence: 1,
                        taskId: "t1",
                        streamId: "pty-1",
                        source: .pty,
                        kind: .stdoutLine,
                        text: "old\u{1B}[",  // dies mid-CSI
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
                        exitCode: 1,
                        timedOut: false
                    ),
                ],
                [
                    TerminalStreamEvent(
                        sequence: 3,
                        taskId: "t1",
                        streamId: "pty-1-r2",  // the fake's deterministic reopen id
                        source: .pty,
                        kind: .stdoutLine,
                        text: "welcome",
                        data: nil,
                        exitCode: nil,
                        timedOut: false
                    )
                ],
            ]
        )
        let model = TerminalSessionModel(descriptor: Self.workspaceDescriptor(), client: client)
        await model.startIfNeeded()
        let closed = await Self.waitUntil { model.availability == .closed }
        XCTAssertTrue(closed, "the first session must end")

        await model.restart()
        let rendered = await Self.waitUntil { model.buffer.plainText.contains("welcome") }
        XCTAssertTrue(
            rendered,
            "a parser stuck in the dead session's CSI swallows the new shell's output; got \(model.buffer.plainText)"
        )
        XCTAssertTrue(model.buffer.plainText.contains("old"), "the transcript itself is kept")

        await model.close()
    }

    /// The route's initial command belongs to OPENING the terminal, not to
    /// every shell: a restart that replays `./deploy.sh` is a second deploy
    /// nobody asked for.
    func testRestartDoesNotReplayTheInitialCommand() async {
        let client = Self.readyClient(
            eventBatches: [[Self.errorEvent(streamID: "pty-1", text: "boom")]]
        )
        var descriptor = Self.workspaceDescriptor()
        descriptor.initialCommand = "echo hi"
        let model = TerminalSessionModel(descriptor: descriptor, client: client)

        await model.startIfNeeded()
        let wrote = await Self.waitUntil { model.history.contains("echo hi") }
        XCTAssertTrue(wrote, "the first open must run the initial command")
        let failed = await Self.waitUntil { model.canRestart }
        XCTAssertTrue(failed, "fixture must reach a restartable state")

        await model.restart()
        XCTAssertEqual(model.availability, .ready)
        // Let any (buggy) replay task get its turn before counting.
        for _ in 0..<5 { await Task.yield() }
        let writes = await client.recordedWrites()
        XCTAssertEqual(
            writes.filter { $0 == "echo hi\n" }.count, 1,
            "restart must not replay the initial command, wrote: \(writes)"
        )

        await model.close()
    }

    private static func errorEvent(streamID: String, text: String) -> TerminalStreamEvent {
        TerminalStreamEvent(
            sequence: 1,
            taskId: "t1",
            streamId: streamID,
            source: .pty,
            kind: .error,
            text: text,
            data: nil,
            exitCode: nil,
            timedOut: false
        )
    }

    /// Polls a MainActor condition instead of sleeping a fixed interval: the
    /// polling loop is a detached MainActor hopper, and on a loaded CI runner
    /// a wall-clock sleep loses the race and files a lifecycle regression
    /// that is really a synchronization bug in the test.
    @MainActor
    private static func waitUntil(
        timeout: Duration = .seconds(2),
        _ condition: @MainActor () -> Bool
    ) async -> Bool {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while clock.now < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(5))
        }
        return condition()
    }

    /// The terminal and the Linux runtime page must install into, and boot,
    /// the same directory. The old fallback hung off `appSandboxRoot`, which
    /// carries a `LingxiCode` component the Settings path does not.
    func testManagedRootFallbackMatchesTheRuntimePageInstallPath() throws {
        let descriptor = TerminalRuntimeDescriptor.make(
            appSandboxRoot: "/some/sandbox/LingxiCode",
            project: nil,
            linuxRuntime: LinuxRuntimeState()
        )

        // Asserted against the shape, not against another call to the same
        // function — `managedRootPath()` has a documented failure branch that
        // returns "", and comparing it to itself would pass on "" while the
        // terminal quietly pointed at nothing.
        let managedRoot = try XCTUnwrap(descriptor.config).managedRoot
        XCTAssertTrue(managedRoot.hasPrefix("/"), "must be absolute, got \(managedRoot)")
        XCTAssertTrue(managedRoot.hasSuffix("mobile-linux/ios-ish"), "got \(managedRoot)")
        XCTAssertFalse(managedRoot.contains("/some/sandbox"), "must not hang off appSandboxRoot")
        XCTAssertEqual(managedRoot, LXISHDefaultWorkspace.managedRootPath())
    }

    /// `LinuxRuntimeState.mounts` carries REAL mount specs only, so the
    /// terminal ships it to `openPty` verbatim. The old design synthesized an
    /// "App Sandbox" display-label row per writable guest path into `mounts`
    /// and re-filtered them here BY STRING SHAPE (`hasPrefix("/")` over a
    /// LOCALIZED label — an invariant held only by whichever translations the
    /// test host happened to run). The display rows are now a Settings-view
    /// rendering of `writableGuestPaths`; this pins that the probe's guest
    /// paths never leak into the descriptor's mount table.
    func testWritableGuestPathsNeverLeakIntoTheRealMountTable() {
        var runtime = LinuxRuntimeState()
        runtime.writableGuestPaths = ["/root", "/tmp", "/var/tmp"]
        runtime.mounts = [
            LinuxRuntimeMountRow(id: "/x", hostPath: "/var/mobile/real", guestPath: "/x", readOnly: true)
        ]

        let descriptor = TerminalRuntimeDescriptor.make(
            appSandboxRoot: "/some/sandbox",
            project: nil,
            linuxRuntime: runtime
        )

        XCTAssertEqual(
            descriptor.mounts.map(\.hostPath), ["/var/mobile/real"],
            "real mounts pass through verbatim; writableGuestPaths stay a display concern"
        )
    }

    private static func workspaceDescriptor() -> TerminalRuntimeDescriptor {
        TerminalRuntimeDescriptor(
            config: nil,
            workspace: TerminalWorkspaceDescriptor(
                hostPath: nil,
                guestPath: "/workspace/project",
                displayName: "Project"
            ),
            pollInterval: .milliseconds(1),
            taskRefreshInterval: 1
        )
    }

    private static func readyClient(
        eventBatches: [[TerminalStreamEvent]] = [],
        closePtyDelay: Duration = .zero
    ) -> FakeTerminalRuntimeClient {
        FakeTerminalRuntimeClient(
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
            eventBatches: eventBatches,
            closePtyDelay: closePtyDelay
        )
    }

    func testHealthyAndUnrecoverableStatesOfferNoRepair() {
        // `.closed`/`.failed` moved OUT of this list on purpose: they map to
        // `.restartSession` now (see testStoppedStatesMapToRestartAnd…). The
        // old version of this test kept asserting they offer nothing, green,
        // while the view grew a restart link from a second table — a
        // green-but-lying document of the fork.
        let states: [TerminalAvailabilityState] = [
            .idle, .opening, .ready,
            .invalidRequest("bad cwd"),
        ]
        for state in states {
            XCTAssertEqual(
                TerminalRecovery.forState(state), TerminalRecovery.none,
                "\(state) must not offer a repair button"
            )
        }
    }
}
