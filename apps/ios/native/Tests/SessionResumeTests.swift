// SessionResumeTests.swift — live ResumeSession (iOS).
//
// Unit coverage for the inbound `SessionResumed` mapping the engine now carries
// the restored transcript on (`SessionResumed { session_id, messages }`). The
// iOS analog of Android's `SessionStateTest` mapper test: it drives a synthetic
// `ClientEvent.sessionResumed` through `EngineConversationSource.apply` (via the
// `applyForTesting` seam) and asserts the out-of-band session-state path both
// adopts the session id AND surfaces the restored conversation — so the chat
// scrollback shows the prior turns the next message will continue from.
//
// No engine, no key, no network: this constructs the lowered DTOs directly and
// exercises only the Swift mapping, so it is hermetic and fast.

import Combine
import XCTest

@testable import LingxiCode

#if canImport(harness_runtimeFFI)
    import harness_runtimeFFI
#endif

#if canImport(harness_runtimeFFI)

    @MainActor
final class SessionResumeTests: XCTestCase {
        func testGlobalEventCenterPublishesOnlyTheSelectedOwner() {
            let center = ClientEventCenter()
            let a = NSObject()
            let b = NSObject()
            var models: [String] = []
            center.subscribe { event in
                if case let .modelList(_, current, _, _) = event { models.append(current) }
            }
            center.publish(.modelList(models: ["model-a"], current: "model-a", currentModelSource: nil, details: []),
                           owner: a, selectedOwner: b)
            center.publish(.modelList(models: ["model-b"], current: "model-b", currentModelSource: nil, details: []),
                           owner: b, selectedOwner: b)
            XCTAssertEqual(models, ["model-b"])
        }

        func testReportDeliveryBindingsRoundTripWithoutShiftingAgentFields() throws {
            for delivery in [nil, "pending", "delivered", "flagged", "withheld"] as [String?] {
                let agent = SessionAgentSummaryDto(reportDelivery: delivery,
                    agentId: "agent:binding-test", name: sourceText("测试 agent"), agentType: "explorer",
                    model: "subscription-model", modelProfile: "openai-chatgpt", status: "completed",
                    latestActivity: sourceText("Done"), updatedAtMs: 123456789)
                let buffer = FfiConverterTypeSessionAgentSummaryDto.lower(agent)
                XCTAssertEqual(try FfiConverterTypeSessionAgentSummaryDto.lift(buffer), agent)
            }
        }


        private func agentRow(index: UInt64, message: MessageDto) -> SessionAgentMessageRowDto {
            SessionAgentMessageRowDto(messageIndex: index, messageUuid: "fixture-row-\(index)", message: message, apiErrorJson: nil)
        }

        private func agentRows(_ messages: [MessageDto]) -> [SessionAgentMessageRowDto] {
            messages.enumerated().map { agentRow(index: UInt64($0.offset), message: $0.element) }
        }


        /// A minimal `EngineConfig` for an `EngineConversationSource` under test —
        /// keyless and rooted at a throwaway temp dir. The source is NEVER asked to
        /// build a handle here; we only drive `applyForTesting`, so no engine is
        /// spun up and no network/key is touched.
        private func makeSource() -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                model: "",
                appSandboxRoot: NSTemporaryDirectory(),
                                sessionMode: .code,
visionDelegationEnabled: true)
            return EngineConversationSource(config: config)
        }

        private func uuid() -> String { "22222222-2222-4222-8222-222222222222" }

        private func enableResume(_ source: EngineConversationSource, ids: [String],
                                  submit: @escaping (ClientCommand) async throws -> Void = { _ in }) {
            source.model.engineSessions = ids.map {
                EngineSession(id: $0, title: $0, mode: .code, messageCount: 0, modifiedAt: nil,
                              relativeTime: "", transcriptPath: "/fixture/\($0).jsonl")
            }
            source.setResumeSubmitterForTesting { id, path in
                XCTAssertEqual(path, "/fixture/\(id).jsonl")
                try await submit(.resumeSession(sessionId: id, cwd: nil, expectedMode: .code))
            }
        }

        private func sourceText(_ value: String) -> ClientTextDto {
            ClientTextDto(codeUnits: Array(value.utf16))
        }

        private func existingSource() -> EngineConversationSource {
            let source = makeSource()
            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: [
                MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: sourceText("history"))])
            ]))
            return source
        }

        func testSessionModeCannotChangeAfterCreation() {
            let source = existingSource()
            source.applyForTesting(.sessionStarted(sessionId: uuid(), mode: .chat))
            XCTAssertEqual(source.model.sessionMode, .code)
            XCTAssertEqual(source.model.activeSessionId, uuid())
            XCTAssertEqual(source.model.messages.first?.text, "history")
        }

        func testAuthoringAdmissionWaitsForExactOwnerWithoutBlockingAnotherSource() async throws {
            let other = makeSource()
            let session = uuid()
            let project = UUID().uuidString.lowercased()
            let cwd = MobileUserStorage.directories().appsGuestPath + "/authoring-test"
            let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: NSTemporaryDirectory(),
                                      projectCwd: cwd, sessionMode: .code, visionDelegationEnabled: true,
                                      projectMountGrants: [cwd])
            let source = EngineConversationSource(config: config)
            let owner = LocalAppAuthoringOwner(sourceDirectory: cwd, projectID: project, sessionID: session, appID: nil)
            let binding = LocalAppAuthoring(sourceDirectory: cwd, workspaceId: project,
                                            projectRef: .mobileProject(project), authoringSessionId: session)
            try source.requireAuthoringBindingBeforeTurns(LocalAppProvisionalAuthoringOwner(
                sourceDirectory: cwd, projectID: project, appID: nil))
            source.model.activeSessionId = session
            source.model.workspaceState = SessionWorkspaceState(sessionId: session, cwd: cwd,
                projectRoot: cwd, additionalDirectories: [], transcriptPath: "/fixture/session.jsonl")
            var submitted = 0
            source.setCommandSubmitterForTesting { _ in submitted += 1 }
            XCTAssertFalse(source.isAvailableForPrompts)
            XCTAssertTrue(other.isAvailableForPrompts)
            let rejected = await source.send("keep this draft")
            XCTAssertNil(rejected)
            do {
                try await source.submitEngineCommand(.sendPrompt(text: sourceText("bypass"), promptMode: nil,
                                                                  images: [], turnId: 1, visualizationContext: nil))
                XCTFail("A direct command must not bypass pending authoring admission")
            } catch {}
            XCTAssertEqual(submitted, 0)
            var wrong = binding
            wrong.authoringSessionId = UUID().uuidString
            XCTAssertThrowsError(try source.confirmAuthoringBinding(owner, binding: wrong))
            let wrongOwner = LocalAppAuthoringOwner(sourceDirectory: cwd, projectID: UUID().uuidString,
                                                     sessionID: session, appID: nil)
            let wrongBinding = LocalAppAuthoring(sourceDirectory: cwd, workspaceId: wrongOwner.projectID,
                projectRef: .mobileProject(wrongOwner.projectID), authoringSessionId: session)
            XCTAssertThrowsError(try source.confirmAuthoringBinding(wrongOwner, binding: wrongBinding))
            XCTAssertFalse(source.isAvailableForPrompts)
            try source.confirmAuthoringBinding(owner, binding: binding)
            XCTAssertTrue(source.isAvailableForPrompts)
            try await source.submitEngineCommand(.sendPrompt(text: sourceText("admitted"), promptMode: nil,
                                                              images: [], turnId: 2, visualizationContext: nil))
            XCTAssertEqual(submitted, 1)
            source.model.activeSessionId = UUID().uuidString
            XCTAssertFalse(source.isAvailableForPrompts)
            do {
                try await source.submitEngineCommand(.sendPrompt(text: sourceText("wrong owner"), promptMode: nil,
                                                                  images: [], turnId: 3, visualizationContext: nil))
                XCTFail("A late owner change must not reuse an earlier binding")
            } catch {}
            XCTAssertEqual(submitted, 1)
        }

        func testAuthoringWorkspaceValidationDrainsAdmittedSubmitAndKeepsOtherOwnersUsable() async throws {
            let source = makeSource()
            let other = makeSource()
            let gate = EngineSubmitGate()
            source.setCommandSubmitterForTesting { command in try await gate.submit(command) }
            let submitted = Task { @MainActor in
                try await source.submitEngineCommand(.sendPrompt(text: sourceText("first"), promptMode: nil,
                    images: [], turnId: 1, visualizationContext: nil))
            }
            await gate.waitForFirstSubmit()
            XCTAssertThrowsError(try source.beginAuthoringWorkspaceValidation())
            await gate.releaseFirstSubmit()
            try await submitted.value
            let token = try source.beginAuthoringWorkspaceValidation()
            XCTAssertFalse(source.isAvailableForPrompts)
            XCTAssertTrue(other.isAvailableForPrompts)
            let rejected = await source.send("keep this draft")
            XCTAssertNil(rejected)
            do {
                try await source.setWorkspaceDirectory("/different", expectedSessionID: uuid())
                XCTFail("A workspace change must not overtake source verification")
            } catch {}
            source.endAuthoringWorkspaceValidation(token)
            XCTAssertTrue(source.isAvailableForPrompts)
        }

        func testColdAuthoringAuthorizationReconnectsSettingsForTheSameOwner() async throws {
            let session = uuid()
            let project = UUID().uuidString.lowercased()
            let cwd = MobileUserStorage.directories().appsGuestPath + "/settings-owner"
            let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: NSTemporaryDirectory(),
                projectCwd: cwd, sessionMode: .code, visionDelegationEnabled: true,
                initialSessionId: session, initialSessionTranscriptPath: "/fixture/session.jsonl")
            let handle = TestMobileEngineHandle(sessionID: session) { _ in }
            let source = EngineConversationSource(config: config, handleBuilder: { _, _, _, _ in handle })
            source.requireAuthoringMountAuthorization()
            let settings = DesktopSettingsRepository.shared
            let initialGeneration = settings.sourceGeneration
            source.setSettingsActive(true)
            defer { source.setSettingsActive(false) }
            for _ in 0..<200 {
                if settings.sourceGeneration >= initialGeneration + 2 { break }
                try await Task.sleep(for: .milliseconds(5))
            }
            XCTAssertGreaterThanOrEqual(settings.sourceGeneration, initialGeneration + 2,
                "The first settings attach must finish while authorization is pending")
            XCTAssertFalse(settings.connected)
            let owner = LocalAppAuthoringOwner(sourceDirectory: cwd, projectID: project,
                                               sessionID: session, appID: nil)
            try source.authorizeAuthoringMount(grants: [cwd], owner: owner)
            try await source.prepare()
            source.setSettingsActive(true)
            for _ in 0..<200 {
                if settings.connected { break }
                try await Task.sleep(for: .milliseconds(5))
            }
            XCTAssertTrue(settings.connected, "Authorization must reattach settings for the existing selected owner")
        }

        func testRecoveredAuthoringCannotSubmitBeforeNativeModeAndReceiptAreConfirmed() async throws {
            let session = uuid()
            let project = UUID().uuidString.lowercased()
            let cwd = MobileUserStorage.directories().appsGuestPath + "/cold-owner"
            let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: NSTemporaryDirectory(),
                projectCwd: cwd, sessionMode: .code, visionDelegationEnabled: true,
                initialSessionId: session, initialSessionTranscriptPath: "/fixture/session.jsonl")
            let source = EngineConversationSource(config: config)
            let owner = LocalAppAuthoringOwner(sourceDirectory: cwd, projectID: project, sessionID: session, appID: nil)
            let binding = LocalAppAuthoring(sourceDirectory: cwd, workspaceId: project,
                projectRef: .mobileProject(project), authoringSessionId: session)
            source.requireAuthoringMountAuthorization()
            try source.authorizeAuthoringMount(grants: [cwd], owner: owner)
            XCTAssertFalse(source.isAvailableForPrompts)
            let token = await source.send("preserve")
            XCTAssertNil(token)
            XCTAssertThrowsError(try source.confirmRecoveredAuthoringOwner(owner, binding: binding))
            source.expectSessionResumeForTesting(session)
            source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
            var wrong = binding
            wrong.authoringSessionId = UUID().uuidString
            XCTAssertThrowsError(try source.confirmRecoveredAuthoringOwner(owner, binding: wrong))
            XCTAssertFalse(source.isAvailableForPrompts)
            try source.confirmRecoveredAuthoringOwner(owner, binding: binding)
            XCTAssertTrue(source.isAvailableForPrompts)
        }

        func testRetiredAuthoringOwnerKeepsUUIDUntilNativeDrainIsConfirmed() async throws {
            let session = uuid()
            let project = UUID().uuidString.lowercased()
            let cwd = MobileUserStorage.directories().appsGuestPath + "/drain-owner"
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: root) }
            let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: root.path,
                projectCwd: cwd, sessionMode: .code, visionDelegationEnabled: true,
                initialSessionId: session, initialSessionTranscriptPath: "/fixture/session.jsonl")
            let source = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
            let owner = LocalAppAuthoringOwner(sourceDirectory: cwd, projectID: project, sessionID: session, appID: nil)
            source.requireAuthoringMountAuthorization()
            try source.authorizeAuthoringMount(grants: [cwd], owner: owner)
            source.expectSessionResumeForTesting(session)
            source.applyForTesting(.sessionResumed(sessionId: session, mode: .code, messages: []))
            await source.waitForDurableRecoveryForTesting()
            let host = SessionHost()
            let entry = try host.retain(source: source, scope: .project(project), mode: .code,
                                        sessionID: session, workspace: cwd, draft: "preserved")
            let other = MockConversationSource(sessionMode: .chat)
            let selected = try host.retain(source: other, scope: .global, mode: .chat,
                                           sessionID: "", workspace: "/global", draft: "")
            host.select(selected)
            try await source.retireForReconnect(allowUnconfirmedAuthoring: true)
            let timedOut = await host.releaseRetiredAuthoringSourceAfterDrain(source, entry: entry,
                stillCurrent: { true }, waitForRelease: { false })
            XCTAssertFalse(timedOut)
            XCTAssertTrue(try host.entryForClaim(sessionID: session, scope: .project(project), mode: .code) === entry)
            let staleNavigation = await host.releaseRetiredAuthoringSourceAfterDrain(source, entry: entry,
                stillCurrent: { false }, waitForRelease: { true })
            XCTAssertFalse(staleNavigation)
            XCTAssertTrue(host.entry(for: source) === entry)
            let drained = await host.releaseRetiredAuthoringSourceAfterDrain(source, entry: entry,
                stillCurrent: { true }, waitForRelease: { true })
            XCTAssertTrue(drained)
            XCTAssertNil(host.entry(for: source))
            XCTAssertTrue(host.entry(for: other) === selected)
        }

        func testPreFFIMediaRejectionDoesNotFenceTheNextOwner() async throws {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: root) }
            let project = UUID().uuidString.lowercased()
            let cwd = MobileUserStorage.directories().appsGuestPath + "/preflight"
            var config = EngineConfig(apiBase: "", model: "", appSandboxRoot: root.path,
                                      projectCwd: cwd, sessionMode: .code, visionDelegationEnabled: true)
            config.projectMountGrants = [cwd]
            let source = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
            try source.requireAuthoringBindingBeforeTurns(.init(sourceDirectory: cwd, projectID: project, appID: nil))
            let host = SessionHost()
            host.protectIdentity(of: source, scope: .project(project), mode: .code)
            do {
                try await source.prepare()
                XCTFail("The missing media host must reject before native dispatch")
            } catch {
                XCTAssertEqual((error as NSError).domain, "IOSNativeMediaHost")
            }
            XCTAssertFalse(source.nativeConstructionStarted)
            host.endPreparingClaim(of: source)
            let retry = MockConversationSource(sessionMode: .code)
            XCTAssertTrue(host.claimIdentity(UUID().uuidString.lowercased(), for: retry,
                                             scope: .project(project), mode: .code))
        }

        func testReconnectRetirementPreservesIdleModelAndRejectsNewPrompts() async throws {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: root) }
            let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: root.path,
                                      sessionMode: .code, visionDelegationEnabled: true)
            let source = EngineConversationSource(config: config, recoveryLedger: DurableConversationTurnLedger())
            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: [
                MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: sourceText("history"))])
            ]))
            await source.waitForDurableRecoveryForTesting()
            try await source.retireForReconnect()
            XCTAssertFalse(source.isAvailableForPrompts)
            XCTAssertEqual(source.model.activeSessionId, uuid())
            XCTAssertEqual(source.model.messages.first?.text, "history")
            let submitted = await source.send("do not admit after close")
            XCTAssertNil(submitted)
            try await source.retireForReconnect()
        }

        func testReconnectCannotRetireAdmittedTurn() async {
            let source = existingSource()
            source.model.streaming = true
            do {
                try await source.retireForReconnect()
                XCTFail("A live owner must not be retired")
            } catch {}
            XCTAssertTrue(source.isAvailableForPrompts)
            XCTAssertTrue(source.model.streaming)
        }

        func testStartupResumeAcceptsOnlyConfiguredOwner() {
            let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: NSTemporaryDirectory(),
                                      sessionMode: .code, visionDelegationEnabled: true, initialSessionId: uuid())
            let source = EngineConversationSource(config: config)
            XCTAssertTrue(source.model.sessionTransitionPending)
            source.applyForTesting(.sessionResumed(sessionId: "other", mode: .code, messages: []))
            XCTAssertTrue(source.model.sessionTransitionPending)
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: []))
            XCTAssertEqual(source.model.activeSessionId, uuid())
            XCTAssertFalse(source.model.sessionTransitionPending)
        }

        /// An empty array before the first `SessionList` means "not loaded";
        /// the same array after the event is an authoritative empty catalog.
        /// Persisting code relies on this distinction to avoid erasing a cached
        /// project index when engine startup or listing has not completed yet.
        func testUnmatchedHistoricalToolUseHasNeutralUnknownOutcome() {
            let source = makeSource()
            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: [
                MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.toolUse(id: "missing-result", tool: "Read", inputJson: "{}", header: currentToolHeader())])
            ]))
            let runs = source.model.items.compactMap { item -> ConversationExecutionRun? in
                if case let .run(run) = item { return run }; return nil
            }
            XCTAssertEqual(runs.flatMap(\.tools).map(\.status), [.unknown])
            XCTAssertTrue(source.model.liveTranscriptToolIDs.isEmpty)
            XCTAssertFalse(source.model.requiresBackgroundExecution)
        }

        func testSessionListMarksAuthoritativeEmptyCatalogAsLoaded() {
            let source = makeSource()
            XCTAssertTrue(source.model.engineSessions.isEmpty)
            XCTAssertFalse(source.model.engineSessionsLoaded)

            source.applyForTesting(.sessionList(sessions: []))

            XCTAssertTrue(source.model.engineSessions.isEmpty)
            XCTAssertTrue(source.model.engineSessionsLoaded)
        }

        func testSessionAgentListIsScopedAndSelectable() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let summary = SessionAgentSummaryDto(reportDelivery: "delivered",
                agentId: "agent:child-1",
                name: sourceText("researcher"),
                agentType: "explorer",
                model: "deepseek-flash",
                modelProfile: "deepseek",
                status: "running",
                latestActivity: sourceText("搜索代码"),
                updatedAtMs: 42
            )

            source.applyForTesting(.sessionAgentList(sessionId: "old-session", agents: [summary]))
            XCTAssertEqual(source.model.agentSummaries.map(\.id), [ConversationModel.mainAgentID])

            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [summary]))
            XCTAssertEqual(source.model.agentSummaries.map(\.id), [ConversationModel.mainAgentID, "agent:child-1"])
            let child = source.model.agentSummaries.first { $0.id == "agent:child-1" }
            XCTAssertEqual(child?.model, "deepseek-flash")
            XCTAssertEqual(child?.modelProfile, "deepseek")
            XCTAssertEqual(child?.reportDelivery, "delivered")
            XCTAssertEqual(
                source.model.agentSummaries.first?.name,
                ConversationAgentSummary.main.name
            )
            source.model.upsertAgentSummary(ConversationAgentSummary(
                id: ConversationModel.mainAgentID,
                name: "main",
                agentType: "main",
                status: "idle",
                latestActivity: "12 messages · model"
            ))
            XCTAssertEqual(source.model.agentSummaries.first?.latestActivity, "12 messages · model")
            source.applyForTesting(.sessionAgentUpdated(
                sessionId: "session-a",
                agent: SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: ConversationModel.mainAgentID,
                    name: sourceText("Remote main"),
                    agentType: "main",
                    model: nil,
                    modelProfile: nil,
                    status: "idle",
                    latestActivity: sourceText("12 messages · model"),
                    updatedAtMs: nil
                )
            ))
            XCTAssertEqual(source.model.agentSummaries.first?.name, ConversationAgentSummary.main.name)
            XCTAssertEqual(source.model.agentSummaries.first?.latestActivity, "12 messages · model")
            source.selectAgent("agent:child-1")
            XCTAssertTrue(source.model.isSelectedAgentReadOnly)
            XCTAssertEqual(source.model.selectedAgentSummary?.latestActivity, "搜索代码")
        }

        func testDelayedAgentListCannotOverwriteNewerLiveUpdate() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"

            source.applyForTesting(.sessionAgentUpdated(
                sessionId: "session-a",
                agent: SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("designer"),
                    agentType: "workflow-subagent",
                    model: "deepseek-flash",
                    modelProfile: "deepseek",
                    status: "completed",
                    latestActivity: sourceText("Design complete"),
                    updatedAtMs: 2_000
                )
            ))
            source.applyForTesting(.sessionAgentList(
                sessionId: "session-a",
                agents: [SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("designer"),
                    agentType: "workflow-subagent",
                    model: "claude-opus-4-8",
                    modelProfile: "anthropic",
                    status: "running",
                    latestActivity: sourceText("Starting"),
                    updatedAtMs: 1_000
                )]
            ))

            let child = source.model.agentSummaries.first { $0.id == agentID }
            XCTAssertEqual(child?.status, "completed")
            XCTAssertEqual(child?.latestActivity, "Design complete")
            XCTAssertEqual(child?.model, "deepseek-flash")
            XCTAssertEqual(child?.modelProfile, "deepseek")
            XCTAssertEqual(child?.updatedAtMs, 2_000)
        }

        func testTranscriptDeliveryDoesNotHideTerminalAgentUpdate() {
            for terminalStatus in ["completed", "failed", "cancelled", "killed"] {
                for messageArrivesFirst in [true, false] {
                    let source = makeSource()
                    source.model.activeSessionId = "session-a"
                    let agentID = "agent:preparer"
                    func update(_ status: String, at timestamp: UInt64) {
                        source.applyForTesting(.sessionAgentUpdated(
                            sessionId: "session-a",
                            agent: SessionAgentSummaryDto(reportDelivery: nil,
                                agentId: agentID, name: sourceText("preparer"), agentType: "workflow-subagent",
                                model: nil, modelProfile: nil, status: status,
                                latestActivity: nil, updatedAtMs: timestamp
                            )
                        ))
                    }
                    func deliverMessage() {
                        source.applyForTesting(.sessionAgentMessage(
                            sessionId: "session-a", agentId: agentID, row: agentRow(index: 0, message: MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "Staging failed".clientTextDto)]))
                        ))
                    }
                    update("running", at: 1_000)
                    if messageArrivesFirst { deliverMessage() }
                    update(terminalStatus, at: 2_000)
                    if !messageArrivesFirst { deliverMessage() }
                    let child = source.model.agentSummaries.first { $0.id == agentID }
                    XCTAssertEqual(child?.status, terminalStatus)
                    XCTAssertEqual(child?.updatedAtMs, 2_000, "Transcript delivery must not advance engine ordering")
                    XCTAssertEqual(child?.latestActivity, "Staging failed")

                    // A real resume is newer, while a delayed previous terminal
                    // event must not terminate this resumed generation.
                    update("running", at: 3_000)
                    update(terminalStatus, at: 2_000)
                    XCTAssertEqual(source.model.agentSummaries.first { $0.id == agentID }?.status, "running")
                }
            }
        }

        /// A parked background agent must be able to come back.
        ///
        /// The engine reports claude-code's `completed` for one (it renders as
        /// `done`), where the port used to invent `idle`. `completed` is in the
        /// terminal set, and the terminal rule used to win over ANY later
        /// update — so a resumed agent would have sat at `done` for the rest of
        /// the session while it worked. The rule is an ordering guard, so it
        /// now yields to a NEWER update; the test above still pins the older
        /// one losing.
        func testResumedParkedAgentLeavesItsCompletedStatus() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"

            source.applyForTesting(.sessionAgentUpdated(
                sessionId: "session-a",
                agent: SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("designer"),
                    agentType: "workflow-subagent",
                    model: "deepseek-flash",
                    modelProfile: "deepseek",
                    status: "completed",
                    latestActivity: sourceText("Design complete"),
                    updatedAtMs: 1_000
                )
            ))
            XCTAssertEqual(source.model.agentSummaries.first { $0.id == agentID }?.status,
                           "completed",
                           "precondition: parking reports completed, a terminal word")

            source.applyForTesting(.sessionAgentUpdated(
                sessionId: "session-a",
                agent: SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("designer"),
                    agentType: "workflow-subagent",
                    model: "deepseek-flash",
                    modelProfile: "deepseek",
                    status: "running",
                    latestActivity: sourceText("Reviewing feedback"),
                    updatedAtMs: 2_000
                )
            ))

            let child = source.model.agentSummaries.first { $0.id == agentID }
            XCTAssertEqual(child?.status, "running", "a resumed parked agent runs again")
            XCTAssertEqual(child?.latestActivity, "Reviewing feedback")
            XCTAssertEqual(child?.updatedAtMs, 2_000)
        }

        func testSessionAgentTranscriptReplacesOnlySelectedChildView() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil, agentId: "agent:child-1", name: sourceText("worker"), agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent("agent:child-1")
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: "agent:child-1",
                messages: agentRows([MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "child result".clientTextDto)])]),
                nextMessageIndex: 1,
                revision: 1
            ))

            XCTAssertEqual(source.model.selectedAgentMessages.map(\.text), ["child result"])
            XCTAssertTrue(source.model.isSelectedAgentReadOnly)
            source.selectAgent(ConversationModel.mainAgentID)
            XCTAssertFalse(source.model.isSelectedAgentReadOnly)
        }

        func testLiveChildPushDoesNotPretendTranscriptIsLoaded() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("worker"),
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "working",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))

            // A live tail can beat the durable transcript reply. It must be
            // visible immediately, but remain eligible for a full load when the
            // user selects this child.
            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                row: agentRow(index: 0, message: MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "live tail".clientTextDto)]))
            ))
            XCTAssertFalse(source.model.agentTranscripts[agentID]?.loaded ?? true)

            source.selectAgent(agentID)
            XCTAssertTrue(source.model.isAgentTranscriptLoading)
        }

        func testLiveBeforeSelectionSurvivesStaleFullAgentSnapshot() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            let same = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "same".clientTextDto)])
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("worker"),
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "working",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))

            // The child can emit before the user opens its transcript. Keep it
            // pending by session/agent, otherwise selecting it would reset the
            // queue and a stale snapshot would erase this newer occurrence.
            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                row: agentRow(index: 1, message: same)
            ))
            source.selectAgent(agentID)
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([same]),
                nextMessageIndex: 1,
                revision: 1
            ))

            XCTAssertEqual(source.model.selectedAgentMessages.map(\.text), ["same", "same"])
        }

        func testSessionAgentTranscriptIDsAreStableAndOccurrenceAware() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            let repeated = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "same".clientTextDto)])
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil, agentId: agentID, name: sourceText("worker"), agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent(agentID)

            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([repeated, repeated]),
                nextMessageIndex: 2,
                revision: 1
            ))
            let firstIDs = source.model.selectedAgentMessages.map(\.id)
            XCTAssertEqual(firstIDs.count, 2)
            XCTAssertNotEqual(firstIDs[0], firstIDs[1])

            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([repeated, repeated]),
                nextMessageIndex: 2,
                revision: 1
            ))
            XCTAssertEqual(source.model.selectedAgentMessages.map(\.id), firstIDs)
        }

        func testTranscriptRevisionAcceptsCompactBoundaryWithSameVisibleWatermark() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("worker"),
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "idle",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent(agentID)

            let initial = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "visible".clientTextDto)])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([initial]),
                nextMessageIndex: 1,
                revision: 10
            ))
            XCTAssertEqual(source.model.selectedAgentMessages.count, 1)
            XCTAssertEqual(source.model.agentTranscripts[agentID]?.revision, 10)

            // Compaction can replace the content/details while retaining the
            // same number of visible rows. The raw revision is the only fence
            // that distinguishes this legitimate update from a stale reply.
            let compacted = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                .compactBoundary(messagesBefore: 4, messagesAfter: 1, summary: "fresh compact summary")
            ])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([compacted]),
                nextMessageIndex: 1,
                revision: 11
            ))
            XCTAssertEqual(source.model.selectedAgentMessages.count, 1)
            XCTAssertEqual(source.model.agentTranscripts[agentID]?.revision, 11)
            let summaries = source.model.selectedAgentMessageDetails.values
                .flatMap(\.blocks)
                .compactMap { block -> String? in
                    guard case let .compactBoundary(_, _, summary) = block else { return nil }
                    return summary
                }
            XCTAssertEqual(summaries, ["fresh compact summary"])
        }

        func testOlderTranscriptRevisionCannotReplaceNewerSnapshot() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("worker"),
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "idle",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent(agentID)

            let fresh = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "fresh".clientTextDto)])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([fresh]),
                nextMessageIndex: 1,
                revision: 20
            ))

            let delayed = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "delayed stale".clientTextDto)])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([delayed]),
                nextMessageIndex: 1,
                revision: 19
            ))

            XCTAssertEqual(source.model.selectedAgentMessages.map(\.text), ["fresh"])
            XCTAssertEqual(source.model.agentTranscripts[agentID]?.revision, 20)
        }

        func testHigherRevisionSnapshotCannotEraseLoadedLiveTail() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("worker"),
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "working",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent(agentID)

            let history = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "history".clientTextDto)])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([history]),
                nextMessageIndex: 1,
                revision: 10
            ))
            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                row: agentRow(index: 1, message: MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "live tail".clientTextDto)]))
            ))

            // A hidden/lifecycle record can advance the raw revision before
            // the live visible row. If that older snapshot is delivered late,
            // its lower visible watermark must not erase the indexed tail.
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([history]),
                nextMessageIndex: 1,
                revision: 11
            ))

            XCTAssertEqual(source.model.selectedAgentMessages.map(\.text), ["history", "live tail"])
            XCTAssertEqual(source.model.agentTranscripts[agentID]?.nextMessageIndex, 2)
            XCTAssertEqual(source.model.agentTranscripts[agentID]?.revision, 10)
        }

        func testSessionAgentToolResultUpdatesExistingToolRun() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil, agentId: agentID, name: sourceText("worker"), agentType: "general", model: nil, modelProfile: nil, status: "working", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent(agentID)

            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                row: agentRow(index: 0, message: MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                    .toolUse(id: "tool-1", tool: "Read", inputJson: #"{"path":"a.txt"}"#, header: currentToolHeader())
                ]))
            ))
            let liveRun = source.model.selectedAgentItems.compactMap { item -> ConversationExecutionRun? in
                guard case let .run(run) = item else { return nil }
                return run
            }
            XCTAssertEqual(liveRun.count, 1)
            XCTAssertEqual(liveRun[0].tools.first?.status, .running)
            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                row: agentRow(index: 1, message: MessageDto(transcriptUuid: nil, role: "user", blocks: [
                    .toolResult(id: "tool-1", tool: "Read", resultJson: #"{"content":"ok"}"#, isError: false, display: currentToolDisplay())
                ]))
            ))

            let runs = source.model.selectedAgentItems.compactMap { item -> ConversationExecutionRun? in
                guard case let .run(run) = item else { return nil }
                return run
            }
            XCTAssertEqual(runs.count, 1)
            XCTAssertEqual(runs[0].tools.count, 1)
            XCTAssertEqual(runs[0].tools[0].id, "tool-1")
            XCTAssertEqual(runs[0].tools[0].status, .completed)
            XCTAssertEqual(
                Set(source.model.selectedAgentItems.map(\.id)).count,
                source.model.selectedAgentItems.count
            )
        }

        func testSnapshotAheadOfLiveToolUseCannotRegressCompletedRun() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("worker"),
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "working",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent(agentID)

            let use = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                .toolUse(id: "tool-snapshot", tool: "Read", inputJson: #"{"path":"a.txt"}"#, header: currentToolHeader())
            ])
            let result = MessageDto(transcriptUuid: nil, role: "user", blocks: [
                .toolResult(id: "tool-snapshot", tool: "Read", resultJson: #"{"content":"ok"}"#, isError: false, display: currentToolDisplay())
            ])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([use, result]),
                nextMessageIndex: 2,
                revision: 1
            ))

            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                row: agentRow(index: 1, message: use)
            ))

            let runs = source.model.selectedAgentItems.compactMap { item -> ConversationExecutionRun? in
                guard case let .run(run) = item else { return nil }
                return run
            }
            XCTAssertEqual(runs.count, 1)
            XCTAssertEqual(runs[0].status, .restored)
            XCTAssertEqual(runs[0].tools.first?.status, .completed)
        }

        func testPureAssistantAgentPushesKeepDistinctRunItems() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil, agentId: agentID, name: sourceText("worker"), agentType: "general", model: nil, modelProfile: nil, status: "working", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent(agentID)

            for (index, text) in ["first", "second"].enumerated() {
                source.applyForTesting(.sessionAgentMessage(
                    sessionId: "session-a",
                    agentId: agentID,
                    row: agentRow(index: UInt64(index), message: MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: text.clientTextDto)]))
                ))
            }
            let runs = source.model.selectedAgentItems.compactMap { item -> ConversationExecutionRun? in
                guard case let .run(run) = item else { return nil }
                return run
            }
            XCTAssertEqual(runs.count, 2)
            XCTAssertEqual(Set(runs.map(\.id)).count, 2)
        }

        func testLateAgentTranscriptFailureCannotOverwriteNewSelection() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let first = "agent:first"
            let second = "agent:second"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil, agentId: first, name: sourceText("first"), agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil),
                SessionAgentSummaryDto(reportDelivery: nil, agentId: second, name: sourceText("second"), agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent(first)
            source.selectAgent(second)
            source.model.failAgentTranscript(first, sessionID: "session-a", message: "stale")
            XCTAssertNil(source.model.agentTranscriptError)
            XCTAssertEqual(source.model.selectedAgentID, second)
            XCTAssertTrue(source.model.isAgentTranscriptLoading)
        }

        func testRepeatedAgentLoadsIgnoreFailureFromOlderGeneration() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("worker"),
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "idle",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            let firstRequest = source.model.markAgentTranscriptLoading(
                agentID,
                sessionID: "session-a"
            )
            let secondRequest = source.model.markAgentTranscriptLoading(
                agentID,
                sessionID: "session-a"
            )
            XCTAssertNotEqual(firstRequest, secondRequest)

            source.model.failAgentTranscript(
                agentID,
                sessionID: "session-a",
                message: "stale",
                requestKey: firstRequest
            )
            XCTAssertNil(source.model.agentTranscriptError)
            source.model.failAgentTranscript(
                agentID,
                sessionID: "session-a",
                message: "current",
                requestKey: secondRequest
            )
            XCTAssertEqual(source.model.agentTranscriptError, "current")
        }

        func testOldSessionAgentEventsCannotRepopulateClearedCache() {
            let source = makeSource()
            source.model.activeSessionId = "session-old"
            let agentID = "agent:old"
            source.applyForTesting(.sessionAgentList(sessionId: "session-old", agents: [
                SessionAgentSummaryDto(reportDelivery: nil, agentId: agentID, name: sourceText("old"), agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent(agentID)
            source.model.activeSessionId = "session-new"
            source.model.clearAgentState()

            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-old",
                agentId: agentID,
                messages: agentRows([MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "stale".clientTextDto)])]),
                nextMessageIndex: 1,
                revision: 1
            ))
            XCTAssertTrue(source.model.agentTranscripts.isEmpty)
            XCTAssertEqual(source.model.selectedAgentID, ConversationModel.mainAgentID)
        }

        func testLiveAgentMessageMergesWithTranscriptReplyAndPreservesReportedStatus() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: "agent:child-1",
                    name: sourceText("worker"),
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "idle",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent("agent:child-1")

            let live = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "live tail".clientTextDto)])
            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: "agent:child-1",
                row: agentRow(index: 0, message: live)
            ))
            XCTAssertEqual(source.model.selectedAgentMessages.map(\.text), ["live tail"])
            XCTAssertEqual(source.model.selectedAgentSummary?.status, "idle", "transcript delivery must preserve authoritative lifecycle state")
            source.applyForTesting(.sessionAgentUpdated(sessionId: "session-a", agent: SessionAgentSummaryDto(reportDelivery: nil,
                agentId: "agent:child-1", name: sourceText("worker"), agentType: "general", model: nil,
                modelProfile: nil, status: "working", latestActivity: nil, updatedAtMs: 1
            )))
            XCTAssertEqual(source.model.selectedAgentSummary?.status, "working")

            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: "agent:child-1",
                messages: agentRows([
                    MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "history".clientTextDto)]),
                    live
                ]),
                nextMessageIndex: 2,
                revision: 2
            ))

            XCTAssertEqual(source.model.selectedAgentMessages.map(\.text), ["history", "live tail"])
            XCTAssertEqual(
                source.model.selectedAgentMessages.filter { $0.text == "live tail" }.count,
                1
            )
        }

        func testStaleFullAgentReplyKeepsIndexedLiveSameTextTail() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            let same = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "same".clientTextDto)])
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("worker"),
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "working",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent(agentID)

            // The live event is newer than the snapshot requested at load
            // start. It has the same text as the historical row, so signature
            // occurrence alone would collapse it into the snapshot's index 0.
            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                row: agentRow(index: 1, message: same)
            ))
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: agentRows([same]),
                nextMessageIndex: 1,
                revision: 1
            ))

            XCTAssertEqual(source.model.selectedAgentMessages.map(\.text), ["same", "same"])
            XCTAssertEqual(
                Set(source.model.selectedAgentMessages.map(\.id)).count,
                2,
                "indexed live tail must retain a distinct stable render identity"
            )
        }

        func testSessionIndexPreservesPendingRestoreAndUnlistedActiveSession() {
            XCTAssertFalse(
                ConversationSessionIndexPolicy.shouldSynchronize(
                    transitionPending: true,
                    activeSessionID: "old-session",
                    listedSessionIDs: []
                )
            )
            XCTAssertFalse(
                ConversationSessionIndexPolicy.shouldSynchronize(
                    transitionPending: false,
                    activeSessionID: "new-session",
                    listedSessionIDs: ["old-session"]
                )
            )
            XCTAssertFalse(
                ConversationSessionIndexPolicy.shouldSynchronize(
                    transitionPending: false,
                    restorePending: true,
                    activeSessionID: "saved-session",
                    listedSessionIDs: []
                )
            )
            XCTAssertTrue(
                ConversationSessionIndexPolicy.shouldSynchronize(
                    transitionPending: false,
                    activeSessionID: "saved-session",
                    listedSessionIDs: ["saved-session"]
                )
            )
        }

        func testPendingRestoreRejectsTransientStartupSessionUntilTargetArrives() {
            XCTAssertFalse(
                ConversationSessionRestorePolicy.shouldAdopt(
                    candidateSessionID: "startup-session",
                    pendingRestoreID: "saved-session"
                )
            )
            XCTAssertTrue(
                ConversationSessionRestorePolicy.shouldAdopt(
                    candidateSessionID: "saved-session",
                    pendingRestoreID: "saved-session"
                )
            )
            XCTAssertTrue(
                ConversationSessionRestorePolicy.shouldAdopt(
                    candidateSessionID: "fresh-session",
                    pendingRestoreID: nil
                )
            )
            XCTAssertEqual(
                ConversationSessionRestorePolicy.rollbackSelection(
                    failedSessionID: "new-session",
                    pendingRestoreID: "new-session",
                    activeSessionID: "new-session",
                    confirmedSessionID: "previous-session"
                ),
                "previous-session"
            )
            XCTAssertNil(
                ConversationSessionRestorePolicy.rollbackSelection(
                    failedSessionID: "stale-failure",
                    pendingRestoreID: "new-session",
                    activeSessionID: "new-session",
                    confirmedSessionID: "previous-session"
                )
            )
        }

        func testSessionMutationPolicyKeepsSelectionStableDuringRecoveryOrCancel() {
            XCTAssertFalse(
                ConversationSessionMutationPolicy.allowsCallerMutation(
                    hasInactiveDurableRecovery: true,
                    hasUnresolvedTurnRecovery: true,
                    isCancelling: false
                )
            )
            XCTAssertFalse(
                ConversationSessionMutationPolicy.allowsCallerMutation(
                    hasInactiveDurableRecovery: false,
                    hasUnresolvedTurnRecovery: true,
                    isCancelling: false
                )
            )
            XCTAssertFalse(
                ConversationSessionMutationPolicy.allowsCallerMutation(
                    hasInactiveDurableRecovery: false,
                    hasUnresolvedTurnRecovery: false,
                    isCancelling: true
                )
            )
            XCTAssertTrue(
                ConversationSessionMutationPolicy.allowsCallerMutation(
                    hasInactiveDurableRecovery: false,
                    hasUnresolvedTurnRecovery: false,
                    isCancelling: false
                )
            )
        }

        func testMissingResumeReportsFailureWithoutCreatingReplacement() async {
            let source = makeSource()
            let recorder = SessionTransitionRecorder(failure: .missing)
            source.setCommandSubmitterForTesting { command in
                try await recorder.submit(command)
            }

            source.resumeSession("missing-session")
            let commands = await recorder.snapshot()
            XCTAssertTrue(commands.isEmpty)
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertNotNil(source.model.error)
            XCTAssertEqual(source.model.sessionTransitionFailure?.requestedSessionID, "missing-session")
        }

        func testMissingSelectedTranscriptDoesNotCancelAnAdmittedTurn() async {
            let source = makeSource()
            var commands: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in await MainActor.run { commands.append(command) } }
            source.beginTurnForTesting(turnId: 9, sessionId: "active-session")
            source.model.engineSessions = [EngineSession(id: "selected", title: "Selected", mode: .code,
                messageCount: 1, modifiedAt: nil, relativeTime: "", transcriptPath: nil)]
            source.resumeSession("selected")
            XCTAssertTrue(source.model.streaming)
            XCTAssertEqual(source.model.activeSessionId, "active-session")
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertNotNil(source.model.error)
            XCTAssertTrue(commands.isEmpty)
        }

        func testNonMissingResumeFailureRemainsVisible() async {
            let source = makeSource()
            let recorder = SessionTransitionRecorder(failure: .generic)
            source.setCommandSubmitterForTesting { command in
                try await recorder.submit(command)
            }
            enableResume(source, ids: ["saved-session"]) { command in try await recorder.submit(command) }

            source.resumeSession("saved-session")
            let receivedCommand = await recorder.waitForCommandCount(1)
            XCTAssertTrue(receivedCommand)
            let transitionSettled = await waitForSessionTransitionToSettle(source)
            XCTAssertTrue(transitionSettled)

            let commands = await recorder.snapshot()
            XCTAssertEqual(commands, [.resume])
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertNotNil(source.model.error)
            XCTAssertEqual(
                source.model.sessionTransitionFailure?.requestedSessionID,
                "saved-session"
            )
        }

        func testRejectedResumeDoesNotMatchLegacyErrorText() async {
            let source = makeSource()
            let recorder = SessionTransitionRecorder(failure: .missingRejected)
            source.setCommandSubmitterForTesting { command in
                try await recorder.submit(command)
            }
            enableResume(source, ids: ["missing-session"]) { command in try await recorder.submit(command) }

            source.resumeSession("missing-session")
            let receivedCommands = await recorder.waitForCommandCount(1)
            XCTAssertTrue(receivedCommands)
            let transitionSettled = await waitForSessionTransitionToSettle(source)
            XCTAssertTrue(transitionSettled)

            let commands = await recorder.snapshot()
            XCTAssertEqual(commands, [.resume])
            XCTAssertNotNil(source.model.error)
        }

        func testConfirmedEmptyResumePreservesSessionIDAndMode() async {
            let source = makeSource()
            let recorder = EmptySessionResumeRecorder()
            source.setCommandSubmitterForTesting { command in
                await recorder.submit(command)
            }
            enableResume(source, ids: ["empty-session"]) { command in await recorder.submit(command) }

            source.resumeSession("empty-session")
            let didResume = await recorder.waitForResume()
            XCTAssertTrue(didResume)

            let request = await recorder.snapshot()
            XCTAssertEqual(
                request,
                .resumeSession(sessionId: "empty-session", cwd: nil, expectedMode: .code)
            )
            XCTAssertTrue(source.model.sessionTransitionPending)

            source.applyForTesting(.sessionResumed(sessionId: "empty-session", mode: .code, messages: []))
            XCTAssertEqual(source.model.activeSessionId, "empty-session")
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertEqual(source.model.sessionRefreshRevision, 1)
        }

        func testStartupModeMismatchFailsClosedWithoutAdoptingSession() {
            let source = makeSource()
            source.applyForTesting(.sessionStarted(sessionId: "other", mode: .chat))
            XCTAssertEqual(source.model.activeSessionId, "")
            XCTAssertFalse(source.isAvailableForPrompts)
            XCTAssertNotNil(source.model.error)
            XCTAssertFalse(source.model.sessionTransitionPending)
        }

        func testResumeModeMismatchEndsTransitionWithoutReplacingOwner() {
            let source = makeSource()
            source.expectSessionResumeForTesting("saved")
            source.applyForTesting(.sessionResumed(sessionId: "saved", mode: .chat, messages: []))
            XCTAssertEqual(source.model.activeSessionId, "")
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertFalse(source.isAvailableForPrompts)
            XCTAssertEqual(source.model.sessionTransitionFailure?.requestedSessionID, "saved")
            XCTAssertNotNil(source.model.error)
        }

        func testBootstrapSessionStartedDoesNotClearPendingResume() {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in }
            enableResume(source, ids: ["saved-session"])

            source.resumeSession("saved-session")
            XCTAssertTrue(source.model.sessionTransitionPending)

            source.applyForTesting(.sessionStarted(sessionId: "startup-session", mode: .code))
            XCTAssertTrue(
                source.model.sessionTransitionPending,
                "a delayed bootstrap SessionStarted is not ResumeSession confirmation"
            )

            source.applyForTesting(.sessionResumed(sessionId: "saved-session", mode: .code, messages: []))
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertEqual(source.model.activeSessionId, "saved-session")
        }

        func testSameTargetSessionStartedCannotCompleteResumeBeforeTranscriptReplay() {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in }
            enableResume(source, ids: ["saved-session"])
            source.model.activeSessionId = "saved-session"

            source.resumeSession("saved-session")
            XCTAssertTrue(source.model.sessionTransitionPending)

            source.applyForTesting(.sessionStarted(sessionId: "saved-session", mode: .code))

            XCTAssertTrue(
                source.model.sessionTransitionPending,
                "only SessionResumed carries the authoritative transcript replay"
            )
            XCTAssertEqual(source.model.activeSessionId, "saved-session")
        }

        func testSameSessionStartedPreservesSelectedChildTranscript() {
            let source = makeSource()
            let sessionID = "session-a"
            let agentID = "agent:design"
            source.model.activeSessionId = sessionID
            source.applyForTesting(.sessionAgentList(sessionId: sessionID, agents: [
                SessionAgentSummaryDto(reportDelivery: nil,
                    agentId: agentID,
                    name: sourceText("design"),
                    agentType: "designer",
                    model: "deepseek-flash",
                    modelProfile: "deepseek",
                    status: "running",
                    latestActivity: sourceText("Inspecting layout"),
                    updatedAtMs: 10
                )
            ]))
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: sessionID,
                agentId: agentID,
                messages: agentRows([MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "Design notes".clientTextDto)])]),
                nextMessageIndex: 1,
                revision: 1
            ))
            source.selectAgent(agentID)

            source.applyForTesting(.sessionStarted(sessionId: sessionID, mode: .code))

            XCTAssertEqual(source.model.selectedAgentID, agentID)
            XCTAssertEqual(source.model.agentSummaries.map(\.id), [
                ConversationModel.mainAgentID,
                agentID,
            ])
            XCTAssertEqual(source.model.agentTranscripts[agentID]?.messages.map(\.text), [
                "Design notes"
            ])
        }

        func testLateSessionResumedCannotOverwriteNewerConfirmedSession() {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in }
            enableResume(source, ids: ["session-b", "session-c"])
            source.model.activeSessionId = "session-a"

            source.resumeSession("session-b")
            source.applyForTesting(.sessionResumed(
                sessionId: "session-b",
                mode: .code,
                messages: [MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "B".clientTextDto)])]
            ))
            source.resumeSession("session-c")
            source.applyForTesting(.sessionResumed(
                sessionId: "session-c",
                mode: .code,
                messages: [MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "C".clientTextDto)])]
            ))

            source.applyForTesting(.sessionResumed(
                sessionId: "session-b",
                mode: .code,
                messages: [MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "late B".clientTextDto)])]
            ))

            XCTAssertEqual(source.model.activeSessionId, "session-c")
            XCTAssertEqual(source.model.messages.map(\.text), ["C"])
        }

        func testUnexpectedBootstrapSessionStartedDoesNotClearVisibleTranscript() {
            let source = makeSource()
            let visibleMessage = Message(role: .ai, text: "keep me visible")
            source.model.activeSessionId = "active-session"
            source.model.messages = [visibleMessage]
            source.model.items = [.message(visibleMessage)]

            source.applyForTesting(.sessionStarted(sessionId: "bootstrap-session", mode: .code))

            XCTAssertEqual(source.model.messages, [visibleMessage])
            XCTAssertEqual(source.model.items, [.message(visibleMessage)])
            XCTAssertEqual(source.model.activeSessionId, "active-session")
            XCTAssertFalse(source.model.isNew)
        }

        func testInitialSessionStartedAdoptsOwnerOfOptimisticFirstPrompt() {
            let source = makeSource()
            let prompt = Message(role: .user, text: "Draw a bicycle")
            source.model.messages = [prompt]
            source.model.items = [.message(prompt)]
            source.model.streaming = true

            source.applyForTesting(.sessionStarted(sessionId: "first-session", mode: .code))

            XCTAssertEqual(source.model.activeSessionId, "first-session")
            XCTAssertEqual(source.model.messages, [prompt])
            XCTAssertEqual(source.model.items, [.message(prompt)])
            XCTAssertTrue(source.model.streaming)
        }

        /// The loaded transition is the persistence trigger. It must never be
        /// observable before the rows carried by the same engine event, or a
        /// crash between the two publications can durably erase the old index.
        func testSessionListPublishesRowsBeforeLoadedTransition() {
            let source = makeSource()
            var sessionIDsObservedWhenLoaded: [String] = []
            let observation = source.model.$engineSessionsLoaded
                .dropFirst()
                .sink { loaded in
                    if loaded {
                        sessionIDsObservedWhenLoaded = source.model.engineSessions.map(\.id)
                    }
                }
            defer { observation.cancel() }

            source.applyForTesting(.sessionList(sessions: [
                SessionRowDto(
                    uuid: uuid(),
                    title: "保留的会话",
                    modifiedRfc3339: "2026-08-02T08:00:00Z",
                    messageCount: 3,
                    mode: .code,
                    path: "/tmp/retained.jsonl",
                    workspacePath: "/home/lingxi/fixture"
                ),
            ]))

            XCTAssertEqual(sessionIDsObservedWhenLoaded, [uuid()])
            XCTAssertEqual(source.model.engineSessions.first?.workspaceDirectory, "/home/lingxi/fixture")
        }

        /// SessionResumed with a 2-message transcript (a user text turn + an
        /// assistant text turn, oldest-first) must: adopt the session id, replace
        /// the transcript with exactly those messages in order, map roles to the
        /// UI user/AI split, and leave the turn idle (not streaming). This proves
        /// the restored scrollback the next turn continues from is visible.
        func testSessionResumedSurfacesRestoredTranscriptOldestFirst() {
            let source = makeSource()
            // Seed a stale placeholder transcript (what `resumeSession` leaves in
            // place before the engine confirms) so we can prove it is replaced.
            source.model.messages = [Message(role: .ai, text: "placeholder")]
            source.model.streaming = true

            let messages: [MessageDto] = [
                MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: "第一条用户消息".clientTextDto)]),
                MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "助手的回复".clientTextDto)]),
            ]
            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: messages))

            XCTAssertEqual(source.model.activeSessionId, uuid(),
                           "SessionResumed must adopt the resumed session id")
            XCTAssertEqual(source.model.messages.count, 2,
                           "the restored transcript must replace the placeholder, one bubble per restored message")
            // Oldest-first order + role split preserved.
            XCTAssertEqual(source.model.messages[0].role, .user)
            XCTAssertEqual(source.model.messages[0].text, "第一条用户消息")
            XCTAssertEqual(source.model.messages[1].role, .ai)
            XCTAssertEqual(source.model.messages[1].text, "助手的回复")
            XCTAssertFalse(source.model.streaming,
                           "a resumed session is idle until the user sends the next turn")
            XCTAssertFalse(source.model.isNew,
                           "a non-empty restored transcript is not the empty-state")
        }

        /// An assistant message carrying multiple block kinds must SPLIT: the
        /// narrative blocks remain message rows in wire order, while reasoning
        /// and tools stay in the execution timeline. Nothing may silently vanish
        /// or be rendered twice.
        func testSessionResumedSplitsRichAssistantBlocks() {
            let source = makeSource()
            let assistant = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                .text(text: "正文".clientTextDto),
                .thinking(thinking: "推理", signature: nil),
                .compactBoundary(messagesBefore: 8, messagesAfter: 2,
                                 summary: "hidden compact summary"),
                .toolUse(id: "t1", tool: "Read", inputJson: "{\"path\":\"a\"}", header: currentToolHeader()),
                .toolResult(id: "t1", tool: "", resultJson: "\"ok\"",
                            isError: false, display: currentToolDisplay()),
            ])
            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: [assistant]))

            XCTAssertEqual(source.model.messages.count, 2)
            let text = source.model.messages.map(\.text).joined(separator: "\n")
            XCTAssertTrue(text.contains("正文"), "text block body must be present")
            XCTAssertFalse(text.contains("推理"), "thinking belongs to the timeline, not the message bubble")
            XCTAssertTrue(text.contains("对话已压缩"),
                          "compact boundary must remain visible after resume")
            XCTAssertFalse(text.contains("hidden compact summary"),
                           "internal compact summary must not be rendered as user text")
            XCTAssertEqual(source.model.messages[0].role, .ai,
                           "assistant role maps to the AI side")

            // The tool_use/tool_result pair becomes ONE dedicated row, not text
            // inside the bubble and not a second row.
            let restoredRuns: [ConversationExecutionRun] = source.model.items.compactMap {
                if case let .run(run) = $0 { return run }
                return nil
            }
            XCTAssertEqual(restoredRuns.count, 1, "the pair merges onto one terminal run")
            XCTAssertEqual(restoredRuns.first?.tools.first?.id, "t1")
            XCTAssertEqual(restoredRuns.first?.tools.first?.tool, "Read")
            XCTAssertEqual(restoredRuns.first?.tools.first?.status, .completed,
                           "the tool_result settles the row its tool_use opened")
            XCTAssertEqual(
                restoredRuns.first?.activities.compactMap { activity -> String? in
                    if case let .tool(id) = activity { return id }
                    return nil
                },
                ["t1"],
                "restored tool activity keeps the wire block order"
            )
            XCTAssertEqual(
                source.model.visibleTimelineGroups.flatMap(\.rows).filter {
                    if case .reasoning = $0 { return true }
                    return false
                }.count,
                1
            )
            let timelineOrder = source.model.visibleTimelineGroups.flatMap(\.rows).map { row -> String in
                switch row {
                case let .message(message):
                    return message.text.contains("正文") ? "message:text" : "message:compact"
                case .reasoning: return "reasoning"
                case .tool: return "tool"
                case .notice: return "notice"
                case .commandOutput: return "command-output"
                }
            }
            XCTAssertEqual(timelineOrder, ["message:text", "reasoning", "message:compact", "tool"])
        }

        func testSessionResumedReconstructsTerminalRunAfterItsFinalAssistantMessage() {
            let source = makeSource()
            let messages: [MessageDto] = [
                MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: "执行检查".clientTextDto)]),
                MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                    .text(text: "开始".clientTextDto),
                    .toolUse(
                        id: "tool-restore",
                        tool: "Read",
                        inputJson: #"{"file_path":"a.rs"}"#,
                        header: currentToolHeader()),
                ]),
                MessageDto(transcriptUuid: nil, role: "user", blocks: [
                    .toolResult(
                        id: "tool-restore",
                        tool: "Read",
                        resultJson: #"{"result":"ok"}"#,
                        isError: false,
                        display: ToolResultDisplayDto(
                            headline: "Added 1 line",
                            headlineKind: .added,
                            headlineArgs: [1],
                            diff: StructuredDiffDto(
                                filePath: "src/host.rs",
                                language: "rust",
                                gutterWidth: 2,
                                additions: 1,
                                removals: 0,
                                truncatedRows: 1,
                                rows: [
                                    DiffRowDto(
                                        kind: .add,
                                        lineNo: 8,
                                        hunk: 3,
                                        wordDiffed: false,
                                        segments: [
                                            CodeSegmentDto(
                                                text: "new line",
                                                class: .plain,
                                                rgb: nil,
                                                bold: false,
                                                italic: false,
                                                underline: false,
                                                emph: false
                                            ),
                                        ]
                                    ),
                                ]
                            ),
                            body: "new line",
                            bodyLines: 1,
                            bodyTruncated: true,
                            collapsed: true
                        )),
                ]),
                MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "检查完成".clientTextDto)]),
            ]

            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: messages))

            let kinds = source.model.items.map { item -> String in
                switch item {
                case .message: return "message"
                case .commandOutput: return "command-output"
                case .run: return "run"
                case .notice: return "notice"
                case .toolCall: return "tool"
                }
            }
            XCTAssertEqual(kinds, ["message", "message", "run", "message"])
            guard case let .run(run)? = source.model.items.first(where: {
                if case .run = $0 { return true }
                return false
            }) else {
                return XCTFail("the restored turn must retain its terminal result")
            }
            XCTAssertEqual(run.status, .restored)
            XCTAssertEqual(run.tools.map(\.id), ["tool-restore"])
            XCTAssertEqual(run.tools.first?.status, .completed)
            XCTAssertEqual(run.tools.first?.display?.diff?.filePath, "src/host.rs")
            XCTAssertEqual(run.tools.first?.display?.diff?.rows.first?.hunk, 3)
            XCTAssertEqual(run.tools.first?.display?.diff?.truncatedRows, 1)
        }

        func testSessionResumedPreservesNarrativeToolInterleavingWithinOneAssistantMessage() {
            let source = makeSource()
            let messages: [MessageDto] = [
                MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                    .text(text: "A".clientTextDto),
                    .toolUse(id: "tool-1", tool: "Read", inputJson: #"{"path":"a"}"#, header: currentToolHeader()),
                    .toolResult(id: "tool-1", tool: "Read", resultJson: #"{"result":"a"}"#, isError: false, display: currentToolDisplay()),
                    .text(text: "B".clientTextDto),
                    .toolUse(id: "tool-2", tool: "Edit", inputJson: #"{"path":"b"}"#, header: currentToolHeader()),
                    .toolResult(id: "tool-2", tool: "Edit", resultJson: #"{"result":"b"}"#, isError: false, display: currentToolDisplay()),
                    .text(text: "C".clientTextDto),
                ]),
            ]

            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: messages))

            let order = source.model.visibleTimelineGroups.flatMap(\.rows).map { row -> String in
                switch row {
                case let .message(message): return "message:\(message.text)"
                case let .tool(_, trace): return "tool:\(trace.id)"
                case .reasoning: return "reasoning"
                case .notice: return "notice"
                case .commandOutput: return "command-output"
                }
            }
            XCTAssertEqual(order, [
                "message:A",
                "tool:tool-1",
                "message:B",
                "tool:tool-2",
                "message:C",
            ])
        }

        func testSessionResumedDoesNotPromoteAToolOutcomeToTheAgentOutcome() {
            let source = makeSource()
            let messages: [MessageDto] = [
                MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: "继续尝试".clientTextDto)]),
                MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                    .toolUse(
                        id: "cancelled-tool",
                        tool: "Shell",
                        inputJson: #"{"command":"long job"}"#,
                        header: currentToolHeader()),
                ]),
                MessageDto(transcriptUuid: nil, role: "user", blocks: [
                    .toolResult(
                        id: "cancelled-tool",
                        tool: "Shell",
                        resultJson: #"{"error":"interrupted","tool_denial_kind":"interrupted"}"#,
                        isError: true,
                        display: currentToolDisplay()),
                ]),
                MessageDto(transcriptUuid: nil, role: "assistant", blocks: [.text(text: "已改用其他方法".clientTextDto)]),
            ]

            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: messages))

            let runs = source.model.items.compactMap { item -> ConversationExecutionRun? in
                guard case let .run(run) = item else { return nil }
                return run
            }
            XCTAssertEqual(runs.first?.status, .restored)
            XCTAssertEqual(
                runs.first?.tools.first?.status,
                .cancelled,
                "the wire-preserved tool outcome keeps its own color")
        }

        /// THE SCROLLBACK BUG: in the Anthropic protocol a `tool_result` block
        /// lives in the USER turn. Restoring one message per `MessageDto` put it
        /// in a right-aligned user bubble whose renderer only draws
        /// `message.text` — so every restored tool result was both misplaced and
        /// invisible. A user turn made ONLY of tool results must produce no user
        /// bubble at all.
        func testSessionResumedNeverRendersAToolResultAsAUserBubble() {
            let source = makeSource()
            let assistant = MessageDto(transcriptUuid: nil, role: "assistant", blocks: [
                .text(text: "我来读一下".clientTextDto),
                .toolUse(id: "t9", tool: "Read", inputJson: "{\"file_path\":\"a.rs\"}", header: currentToolHeader()),
            ])
            let toolTurn = MessageDto(transcriptUuid: nil, role: "user", blocks: [
                .toolResult(id: "t9", tool: "Read", resultJson: "{\"result\":\"ok\"}",
                            isError: false, display: currentToolDisplay()),
            ])
            let followUp = MessageDto(transcriptUuid: nil, role: "user", blocks: [.text(text: "继续".clientTextDto)])

            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(
                .sessionResumed(sessionId: uuid(), mode: .code, messages: [assistant, toolTurn, followUp]))

            let userMessages = source.model.messages.filter { $0.role == .user }
            XCTAssertEqual(userMessages.count, 1,
                           "the tool-result-only turn must not become a user bubble")
            XCTAssertEqual(userMessages.first?.text, "继续")

            // Order is preserved: the activity starts before assistant text,
            // then the follow-up user message.
            let kinds = source.model.items.map { item -> String in
                switch item {
                case .message: return "message"
                case .commandOutput: return "command-output"
                case .toolCall: return "tool"
                case .run: return "run"
                case .notice: return "notice"
                }
            }
            XCTAssertEqual(kinds, ["run", "message", "message"])
        }

        /// A zero-message resume (a session with no transcript) must still adopt
        /// the id, clear the placeholder, and fall into the empty-state — never
        /// leave a stale placeholder bubble behind.
        func testSessionResumedEmptyTranscriptClearsToEmptyState() {
            let source = makeSource()
            source.model.messages = [Message(role: .ai, text: "placeholder")]

            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: []))

            XCTAssertEqual(source.model.activeSessionId, uuid())
            XCTAssertTrue(source.model.messages.isEmpty,
                          "an empty restored transcript clears the placeholder")
            XCTAssertTrue(source.model.isNew,
                          "a zero-message resume lands in the empty-state")
        }

        /// A `system` role (the engine emits it for system messages) renders on
        /// the AI side (the iOS `Message.role` is the binary user/AI split), never
        /// dropped.
        func testSessionResumedSystemRoleRendersAsAi() {
            let source = makeSource()
            let system = MessageDto(transcriptUuid: nil, role: "system", blocks: [.text(text: "系统提示".clientTextDto)])
            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), mode: .code, messages: [system]))

            XCTAssertEqual(source.model.messages.count, 1)
            XCTAssertEqual(source.model.messages[0].role, .ai)
            XCTAssertEqual(source.model.messages[0].text, "系统提示")
        }

        func testUnreadableCatalogRejectsUnclaimedOwnerBeforeNativeStartupAndSend() async {
            let host = SessionHost()
            var buildCount = 0
            let source = makeSource(handleBuilder: { _, _, _, _ in
                buildCount += 1
                return TestMobileEngineHandle { _ in }
            })
            host.admitsUnclaimedOwner = { false }
            host.protectIdentity(of: source, scope: .global, mode: .code)

            let submitted = await source.send("preserve this draft")
            XCTAssertNil(submitted)
            XCTAssertTrue(source.model.messages.isEmpty)
            await XCTAssertThrowsAsyncError(try await source.prepare())
            XCTAssertEqual(buildCount, 0, "the native constructor must not run")
            XCTAssertNotNil(source.model.error)
        }

        func testRetainedStartupWaitsForReceiptScanAndRechecksOwnerBeforeBuild() async throws {
            let host = SessionHost()
            var buildCount = 0
            let id = uuid()
            let source = makeSource(handleBuilder: { _, _, _, _ in
                buildCount += 1
                return TestMobileEngineHandle(sessionID: id) { _ in }
            })
            var scanComplete = false
            var ownerAllowed = true
            host.startupAdmissionReady = { scanComplete }
            host.admitsUnclaimedOwner = { true }
            host.admitsSessionIdentity = { _, _, _ in ownerAllowed }
            let entry = try host.retain(source: source, scope: .global, mode: .code,
                                        sessionID: id, workspace: "/global", draft: "preserved")
            await XCTAssertThrowsAsyncError(try await source.prepare())
            XCTAssertEqual(buildCount, 0)
            scanComplete = true
            ownerAllowed = false // A newly discovered native receipt owns this UUID elsewhere.
            await XCTAssertThrowsAsyncError(try await source.prepare())
            XCTAssertEqual(buildCount, 0)
            XCTAssertEqual(entry.draft, "preserved")
            ownerAllowed = true
            try await source.prepare()
            XCTAssertEqual(buildCount, 1)
            scanComplete = false
            ownerAllowed = false
            try await source.prepare() // A live handle remains admitted after later index errors.
            XCTAssertEqual(buildCount, 1)
            let submitted = await source.send("Continue the admitted owner")
            XCTAssertNotNil(submitted)
        }

        func testWorkspaceReadUsesNativeOwnerAndDiscardsALateSessionChange() async throws {
            let id = uuid()
            let gate = WorkspaceReadGate(sessionID: id)
            let handle = TestMobileEngineHandle(sessionID: id, workspaceHandler: { await gate.read() }) { _ in }
            let source = makeSource(handleBuilder: { _, _, _, _ in handle })
            try await source.prepare()
            source.model.activeSessionId = id
            source.model.workspaceState = SessionWorkspaceState(sessionId: id, cwd: "/forged",
                projectRoot: "/forged", additionalDirectories: [], transcriptPath: "/other.jsonl")
            let verified = await source.authenticatedWorkspaceState(sessionID: id, mode: .code)
            XCTAssertEqual(verified?.cwd, "/fixture")
            XCTAssertEqual(verified?.transcriptPath, "/fixture/session.jsonl")
            let host = SessionHost()
            let entry = try host.retain(source: source, scope: .global, mode: .code, sessionID: id,
                                        workspace: "/fixture", draft: "")
            let original = await host.validatesCatalogSource(sessionID: id, scope: .global, mode: .code,
                                                              transcriptPath: "/fixture/session.jsonl")
            let copy = await host.validatesCatalogSource(sessionID: id, scope: .global, mode: .code,
                                                          transcriptPath: "/other.jsonl")
            XCTAssertTrue(original)
            XCTAssertFalse(copy, "a catalog row cannot replace the retained native owner's source")
            let wrongMode = await source.authenticatedWorkspaceState(sessionID: id, mode: .chat)
            XCTAssertNil(wrongMode)

            await gate.blockNextRead()
            let snapshotRead = Task { await host.authenticatedWorkspaceSnapshot(for: entry) }
            guard await gate.waitUntilBlocked() else {
                snapshotRead.cancel()
                await gate.releaseRead()
                return XCTFail("the old native workspace read did not start")
            }
            source.model.workspaceState = SessionWorkspaceState(sessionId: id, cwd: "/new",
                projectRoot: "/fixture", additionalDirectories: [], transcriptPath: "/fixture/session.jsonl")
            await gate.setCwd("/new")
            await gate.releaseRead()
            let snapshot = await snapshotRead.value
            XCTAssertEqual(snapshot?.state.cwd, "/new", "a workspace event during the read must force a fresh query")

            await gate.blockNextRead()
            let read = Task { await source.authenticatedWorkspaceState(sessionID: id, mode: .code) }
            guard await gate.waitUntilBlocked() else {
                read.cancel()
                await gate.releaseRead()
                return XCTFail("the native workspace read did not start")
            }
            source.model.activeSessionId = "33333333-3333-4333-8333-333333333333"
            await gate.releaseRead()
            let stale = await read.value
            XCTAssertNil(stale, "a late native reply must not update a different owner")
        }

        func testMovedIndexedOwnerIsRejectedBeforeNativeStartup() async {
            let host = SessionHost()
            var buildCount = 0
            let source = makeSource(handleBuilder: { _, _, _, _ in
                buildCount += 1
                return TestMobileEngineHandle { _ in }
            })
            let id = uuid()
            var indexedScope = ConversationScope.global
            host.admitsUnclaimedOwner = { true }
            host.admitsSessionIdentity = { _, scope, _ in scope == indexedScope }
            host.protectIdentity(of: source, scope: .global, mode: .code, sessionID: id)
            XCTAssertTrue(host.allowsStartup(of: source, sessionID: id, scope: .global, mode: .code))
            indexedScope = .project("other")

            let submitted = await source.send("keep this draft")
            XCTAssertNil(submitted)
            await XCTAssertThrowsAsyncError(try await source.prepare())
            XCTAssertEqual(buildCount, 0, "the conflicting UUID must not start a second native owner")
        }

        func testConcurrentEngineEntryPointsShareOneHandleBuild() async throws {
            let gate = EngineSubmitGate()
            let handle = TestMobileEngineHandle { command in
                try await gate.submit(command)
            }
            var buildCount = 0
            let source = makeSource(handleBuilder: { _, _, _, _ in
                buildCount += 1
                return handle
            })

            source.warmUp()
            source.listSessions()
            let prompt = Task { await source.send("并发消息") }
            let prepare = Task { try await source.prepare() }

            await gate.waitForFirstSubmit()
            XCTAssertEqual(buildCount, 1)
            await gate.releaseFirstSubmit()
            try await prepare.value
            _ = await prompt.value
            let receivedCommands = await gate.waitForSubmitCount(4)
            XCTAssertTrue(receivedCommands)

            XCTAssertEqual(buildCount, 1,
                           "warm-up, prepare, list and send must await the same engine build")
            let commands = await gate.snapshot()
            var sessionListCommandCount = 0
            for command in commands {
                guard case let .listSessions(limit, _) = command else { continue }
                sessionListCommandCount += 1
                XCTAssertEqual(limit, UInt32.max,
                               "bootstrap and drawer refresh must both request the full catalog")
            }
            XCTAssertGreaterThan(sessionListCommandCount, 0)
        }

        func testRetirementWaitsForInFlightBootstrapAndKeepsItsRealUUID() async throws {
            let gate = EngineSubmitGate()
            let sessionID = uuid()
            let handle = TestMobileEngineHandle(sessionID: sessionID) { command in try await gate.submit(command) }
            let source = makeSource(handleBuilder: { _, _, _, _ in handle })
            var fencedEngineIdentity: ObjectIdentifier?
            var beginCalls = 0
            var mediaDrained = false
            source.setMediaDrainForTesting(begin: { engine in
                beginCalls += 1
                fencedEngineIdentity = ObjectIdentifier(engine)
            }, wait: { _ in mediaDrained })
            let preparing = Task { try await source.prepare() }
            await gate.waitForFirstSubmit()
            var firstFinished = false
            var secondFinished = false
            let first = Task { @MainActor in
                try await source.retireForReconnect()
                firstFinished = true
            }
            let second = Task { @MainActor in
                try await source.retireForReconnect()
                secondFinished = true
            }
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while !source.isRetiredForReconnect, clock.now < deadline { await Task.yield() }
            guard source.isRetiredForReconnect else {
                await gate.releaseFirstSubmit()
                _ = try? await preparing.value
                _ = try? await first.value
                _ = try? await second.value
                return XCTFail("retirement did not start")
            }
            XCTAssertFalse(firstFinished)
            XCTAssertFalse(secondFinished)
            await gate.releaseFirstSubmit()
            try await first.value
            try await second.value
            await XCTAssertThrowsAsyncError(try await preparing.value)
            XCTAssertTrue(firstFinished)
            XCTAssertTrue(secondFinished)
            XCTAssertEqual(beginCalls, 1, "Concurrent retirement must fence the constructed engine exactly once")
            XCTAssertEqual(fencedEngineIdentity, ObjectIdentifier(handle),
                           "The drain must use the engine returned by the in-flight build")
            XCTAssertFalse(source.hasPreparedNativeHandle)
            let capturedID = await source.preparedSessionIDForRelease()
            XCTAssertEqual(capturedID, sessionID)
            let timedOut = await source.waitForMediaAttachmentDrain(timeoutMs: 1)
            XCTAssertFalse(timedOut)
            mediaDrained = true
            let drained = await source.waitForMediaAttachmentDrain(timeoutMs: 1)
            XCTAssertTrue(drained)
        }

        func testEngineHandleConstructionDoesNotRunOnMainThread() async throws {
            let constructed = expectation(description: "engine handle constructed off-main")
            let handle = TestMobileEngineHandle { _ in }
            let source = makeSource(handleBuilder: { _, _, _, _ in
                XCTAssertFalse(
                    Thread.isMainThread,
                    "synchronous engine construction must not block the setup UI"
                )
                XCTAssertGreaterThanOrEqual(
                    Thread.current.stackSize, 8 * 1024 * 1024,
                    "MCP bootstrap must not run on a small cooperative worker stack"
                )
                constructed.fulfill()
                return handle
            })

            try await source.prepare()
            await fulfillment(of: [constructed], timeout: 1)
        }

        func testBootstrapRequestsTheCompleteSessionCatalog() async throws {
            let gate = EngineSubmitGate()
            let handle = TestMobileEngineHandle { command in
                try await gate.submit(command)
            }
            let source = makeSource(handleBuilder: { _, _, _, _ in handle })

            let prepare = Task { try await source.prepare() }
            await gate.waitForFirstSubmit()
            await gate.releaseFirstSubmit()
            try await prepare.value
            let receivedCommands = await gate.waitForSubmitCount(2)
            XCTAssertTrue(receivedCommands)

            let commands = await gate.snapshot()
            guard let sessionList = commands.first(where: { command in
                if case .listSessions = command { return true }
                return false
            }) else {
                return XCTFail("engine bootstrap must request the session catalog")
            }
            guard case let .listSessions(limit, _) = sessionList else { return }
            XCTAssertEqual(limit, UInt32.max,
                           "project persistence requires an uncapped catalog")
        }

        func testFailedSharedEngineBuildCanRetryWithoutCachingPartialHandle() async throws {
            let failingGate = EngineSubmitGate(firstSubmitError: EngineBuildTestError.bootstrapFailed)
            let succeedingGate = EngineSubmitGate()
            var buildCount = 0
            let source = makeSource(handleBuilder: { _, _, _, _ in
                buildCount += 1
                if buildCount == 1 {
                    return TestMobileEngineHandle { command in
                        try await failingGate.submit(command)
                    }
                }
                return TestMobileEngineHandle { command in
                    try await succeedingGate.submit(command)
                }
            })

            let first = Task { try await source.prepare() }
            let concurrent = Task { try await source.prepare() }
            await failingGate.waitForFirstSubmit()
            XCTAssertEqual(buildCount, 1, "concurrent prepare calls must share the failing attempt")
            await failingGate.releaseFirstSubmit()

            await XCTAssertThrowsAsyncError(try await first.value)
            await XCTAssertThrowsAsyncError(try await concurrent.value)

            let retry = Task { try await source.prepare() }
            await succeedingGate.waitForFirstSubmit()
            XCTAssertEqual(buildCount, 2, "a failed bootstrap must not leave a cached handle")
            await succeedingGate.releaseFirstSubmit()
            try await retry.value
            XCTAssertEqual(buildCount, 2)
        }

        func testConcurrentPrepareWaitsForOwnerAdmissionAndSharesRejection() async throws {
            let id = uuid()
            let workspace = WorkspaceReadGate(sessionID: id)
            let submissions = EngineSubmitGate()
            let handle = TestMobileEngineHandle(sessionID: id, workspaceHandler: { await workspace.read() }) { command in
                try await submissions.submit(command)
            }
            let source = makeSource(handleBuilder: { _, _, _, _ in handle })
            var leaseActive = false
            var leaseFinishes = 0
            source.setSessionStartupLease(begin: { leaseActive = true; return true }, finish: {
                leaseActive = false; leaseFinishes += 1
            })
            source.setSessionOwnerAdmission { _ in false }
            var firstSettled = false, secondSettled = false
            let first = Task { @MainActor in
                defer { firstSettled = true }
                try await source.prepare()
            }
            await submissions.waitForFirstSubmit()
            await workspace.blockNextRead()
            let second = Task { @MainActor in
                defer { secondSettled = true }
                try await source.prepare()
            }
            await Task.yield()
            await submissions.releaseFirstSubmit()
            guard await workspace.waitUntilBlocked() else {
                await workspace.releaseRead()
                return XCTFail("final owner admission did not reach the workspace read")
            }
            await Task.yield()
            XCTAssertFalse(firstSettled)
            XCTAssertFalse(secondSettled, "a concurrent caller cannot receive the raw, unadmitted handle")
            XCTAssertTrue(leaseActive)
            await workspace.releaseRead()
            await XCTAssertThrowsAsyncError(try await first.value)
            await XCTAssertThrowsAsyncError(try await second.value)
            XCTAssertFalse(source.hasPreparedNativeHandle)
            XCTAssertFalse(leaseActive)
            XCTAssertEqual(leaseFinishes, 1)
        }

        func testFirstPromptCheckpointFailureDoesNotSubmitOrConsumeDraft() async throws {
            let sandbox = FileManager.default.temporaryDirectory.appendingPathComponent("lingxi-first-prompt-\(UUID().uuidString)", isDirectory: true)
            try FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
            addTeardownBlock { try? FileManager.default.removeItem(at: sandbox) }
            let failure = RecoveryWriterFailure()
            failure.set(.beforeLedger)
            let ledger = DurableConversationTurnLedger(writer: RecoveryFailureWriter(failure: failure))
            let commands = EngineCommandRecorder()
            let handle = TestMobileEngineHandle(sessionID: uuid()) { command in await commands.append(command) }
            let config = EngineConfig(apiBase: "", model: "", appSandboxRoot: sandbox.path,
                                      sessionMode: .code, visionDelegationEnabled: false)
            let source = EngineConversationSource(config: config, handleBuilder: { _, _, _, _ in handle }, recoveryLedger: ledger)
            let token = await source.send("keep the first draft")
            XCTAssertNil(token)
            XCTAssertTrue(source.model.messages.isEmpty)
            let submitted = await commands.snapshot()
            XCTAssertFalse(submitted.contains { if case .sendPrompt = $0 { return true }; return false })
            XCTAssertNotNil(source.model.error)
        }

        private func makeSource(
            handleBuilder: @escaping EngineConversationSource.HandleBuilder
        ) -> EngineConversationSource {
            let sandbox = FileManager.default.temporaryDirectory.appendingPathComponent("lingxi-resume-\(UUID().uuidString)", isDirectory: true)
            try? FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
            addTeardownBlock { try? FileManager.default.removeItem(at: sandbox) }
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                model: "",
                appSandboxRoot: sandbox.path,
                                sessionMode: .code,
visionDelegationEnabled: true)
            return EngineConversationSource(config: config, handleBuilder: handleBuilder)
        }

        private func XCTAssertThrowsAsyncError<T>(
            _ expression: @autoclosure () async throws -> T,
            file: StaticString = #filePath,
            line: UInt = #line
        ) async {
            do {
                _ = try await expression()
                XCTFail("expected async operation to throw", file: file, line: line)
            } catch {
                // Expected.
            }
        }

        private func waitForSessionTransitionToSettle(
            _ source: EngineConversationSource
        ) async -> Bool {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while source.model.sessionTransitionPending, clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(1))
            }
            return !source.model.sessionTransitionPending
        }
    }

    private enum EngineBuildTestError: Error {
        case bootstrapFailed
    }

    private enum SessionTransitionTestError: Error {
        case missing
        case missingRejected
        case generic
    }

    private actor SessionTransitionRecorder {
        enum Command: Equatable {
            case resume
            case new
            case other
        }

        private let failure: SessionTransitionTestError
        private var commands: [Command] = []

        init(failure: SessionTransitionTestError) {
            self.failure = failure
        }

        func submit(_ command: ClientCommand) throws {
            switch command {
            case .resumeSession:
                commands.append(.resume)
                switch failure {
                case .missing:
                    throw ClientError.NotFound(message: "Session missing-session was not found.")
                case .missingRejected:
                    throw ClientError.Rejected(
                        message: "resume: session missing-session not resumable: Session missing-session was not found."
                    )
                case .generic:
                    throw ClientError.Transport(message: "resume transport rejected")
                }
            case .newSession:
                commands.append(.new)
            default:
                commands.append(.other)
            }
        }

        func waitForCommandCount(_ count: Int) async -> Bool {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while commands.count < count, clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(1))
            }
            return commands.count >= count
        }

        func snapshot() -> [Command] {
            commands
        }
    }

    private actor EmptySessionResumeRecorder {
        private var request: ClientCommand?

        func submit(_ command: ClientCommand) {
            request = command
        }

        func waitForResume() async -> Bool {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while request == nil, clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(1))
            }
            return request != nil
        }

        func snapshot() -> ClientCommand? {
            request
        }
    }

    private final class TestMobileEngineHandle: MobileEngineHandle, @unchecked Sendable {
        private let submitHandler: (ClientCommand) async throws -> Void
        private let sessionID: String
        private let workspaceHandler: (() async -> WorkspaceStateDto)?

        required init(unsafeFromHandle handle: UInt64) {
            submitHandler = { _ in }
            sessionID = "fixture-session"
            workspaceHandler = nil
            super.init(unsafeFromHandle: handle)
        }

        init(sessionID: String = "fixture-session", workspaceHandler: (() async -> WorkspaceStateDto)? = nil,
             submitHandler: @escaping (ClientCommand) async throws -> Void) {
            self.submitHandler = submitHandler
            self.sessionID = sessionID
            self.workspaceHandler = workspaceHandler
            super.init(noHandle: .init())
        }

        override func workspaceState() async -> WorkspaceStateDto {
            if let workspaceHandler { return await workspaceHandler() }
            return WorkspaceStateDto(sessionId: sessionID, cwd: "/fixture", projectRoot: "/fixture",
                additionalDirectories: [], transcriptPath: "/fixture/session.jsonl")
        }

        override func visualizationHost(origin: String) -> VisualizationHost? { nil }
        override func audioSessionContext() async -> String { "{}" }


        override func submit(command: ClientCommand) async throws {
            try await submitHandler(command)
        }
    }

    private actor WorkspaceReadGate {
        private let sessionID: String
        private var cwd = "/fixture"
        private var block = false
        private var blocked = false
        private var releaseWaiter: CheckedContinuation<Void, Never>?

        init(sessionID: String) { self.sessionID = sessionID }
        func blockNextRead() { block = true; blocked = false }
        func setCwd(_ value: String) { cwd = value }
        func waitUntilBlocked() async -> Bool {
            for _ in 0..<200 {
                if blocked { return true }
                try? await Task.sleep(for: .milliseconds(5))
            }
            return blocked
        }
        func releaseRead() { releaseWaiter?.resume(); releaseWaiter = nil }
        func read() async -> WorkspaceStateDto {
            let capturedCwd = cwd
            if block {
                block = false
                blocked = true
                await withCheckedContinuation { releaseWaiter = $0 }
            }
            return WorkspaceStateDto(sessionId: sessionID, cwd: capturedCwd, projectRoot: "/fixture",
                additionalDirectories: [], transcriptPath: "/fixture/session.jsonl")
        }
    }

    private actor EngineSubmitGate {
        private let firstSubmitError: Error?
        private var commands: [ClientCommand] = []
        private var firstSubmitContinuation: CheckedContinuation<Void, Never>?

        init(firstSubmitError: Error? = nil) {
            self.firstSubmitError = firstSubmitError
        }

        func submit(_ command: ClientCommand) async throws {
            commands.append(command)
            if commands.count == 1 {
                await withCheckedContinuation { continuation in
                    firstSubmitContinuation = continuation
                }
                if let firstSubmitError {
                    throw firstSubmitError
                }
            }
        }

        func waitForFirstSubmit() async {
            _ = await waitForSubmitCount(1)
        }

        func waitForSubmitCount(_ expectedCount: Int) async -> Bool {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while commands.count < expectedCount, clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(1))
            }
            return commands.count >= expectedCount
        }

        func releaseFirstSubmit() {
            firstSubmitContinuation?.resume()
            firstSubmitContinuation = nil
        }

        func snapshot() -> [ClientCommand] {
            commands
        }
    }

    private actor EngineCommandRecorder {
        private var commands: [ClientCommand] = []
        func append(_ command: ClientCommand) { commands.append(command) }
        func snapshot() -> [ClientCommand] { commands }
    }

#endif
