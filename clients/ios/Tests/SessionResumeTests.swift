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

#if canImport(engine_mobileFFI)
    import engine_mobileFFI
#endif

#if canImport(engine_mobileFFI)

    @MainActor
    final class SessionResumeTests: XCTestCase {

        /// A minimal `EngineConfig` for an `EngineConversationSource` under test —
        /// keyless and rooted at a throwaway temp dir. The source is NEVER asked to
        /// build a handle here; we only drive `applyForTesting`, so no engine is
        /// spun up and no network/key is touched.
        private func makeSource() -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory(),
                visionDelegationEnabled: true)
            return EngineConversationSource(config: config)
        }

        private func uuid() -> String { "22222222-2222-4222-8222-222222222222" }

        /// An empty array before the first `SessionList` means "not loaded";
        /// the same array after the event is an authoritative empty catalog.
        /// Persisting code relies on this distinction to avoid erasing a cached
        /// project index when engine startup or listing has not completed yet.
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
            let summary = SessionAgentSummaryDto(
                agentId: "agent:child-1",
                name: "researcher",
                agentType: "explorer",
                model: "deepseek-v4-flash",
                modelProfile: "deepseek",
                status: "running",
                latestActivity: "搜索代码",
                updatedAtMs: 42
            )

            source.applyForTesting(.sessionAgentList(sessionId: "old-session", agents: [summary]))
            XCTAssertEqual(source.model.agentSummaries.map(\.id), [ConversationModel.mainAgentID])

            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [summary]))
            XCTAssertEqual(source.model.agentSummaries.map(\.id), [ConversationModel.mainAgentID, "agent:child-1"])
            let child = source.model.agentSummaries.first { $0.id == "agent:child-1" }
            XCTAssertEqual(child?.model, "deepseek-v4-flash")
            XCTAssertEqual(child?.modelProfile, "deepseek")
            XCTAssertEqual(
                source.model.agentSummaries.first?.name,
                String(localized: "chat_agent_main")
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
                agent: SessionAgentSummaryDto(
                    agentId: ConversationModel.mainAgentID,
                    name: "Remote main",
                    agentType: "main",
                    model: nil,
                    modelProfile: nil,
                    status: "idle",
                    latestActivity: "12 messages · model",
                    updatedAtMs: nil
                )
            ))
            XCTAssertEqual(source.model.agentSummaries.first?.name, String(localized: "chat_agent_main"))
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
                agent: SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "designer",
                    agentType: "workflow-subagent",
                    model: "deepseek-v4-flash",
                    modelProfile: "deepseek",
                    status: "completed",
                    latestActivity: "Design complete",
                    updatedAtMs: 2_000
                )
            ))
            source.applyForTesting(.sessionAgentList(
                sessionId: "session-a",
                agents: [SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "designer",
                    agentType: "workflow-subagent",
                    model: "claude-opus-4-8",
                    modelProfile: "anthropic",
                    status: "running",
                    latestActivity: "Starting",
                    updatedAtMs: 1_000
                )]
            ))

            let child = source.model.agentSummaries.first { $0.id == agentID }
            XCTAssertEqual(child?.status, "completed")
            XCTAssertEqual(child?.latestActivity, "Design complete")
            XCTAssertEqual(child?.model, "deepseek-v4-flash")
            XCTAssertEqual(child?.modelProfile, "deepseek")
            XCTAssertEqual(child?.updatedAtMs, 2_000)
        }

        func testSessionAgentTranscriptReplacesOnlySelectedChildView() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(agentId: "agent:child-1", name: "worker", agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent("agent:child-1")
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: "agent:child-1",
                messages: [MessageDto(role: "assistant", blocks: [.text(text: "child result")])],
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
                SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "worker",
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
                messageIndex: 0,
                message: MessageDto(role: "assistant", blocks: [.text(text: "live tail")])
            ))
            XCTAssertFalse(source.model.agentTranscripts[agentID]?.loaded ?? true)

            source.selectAgent(agentID)
            XCTAssertTrue(source.model.isAgentTranscriptLoading)
        }

        func testLiveBeforeSelectionSurvivesStaleFullAgentSnapshot() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            let same = MessageDto(role: "assistant", blocks: [.text(text: "same")])
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "worker",
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
                messageIndex: 1,
                message: same
            ))
            source.selectAgent(agentID)
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [same],
                nextMessageIndex: 1,
                revision: 1
            ))

            XCTAssertEqual(source.model.selectedAgentMessages.map(\.text), ["same", "same"])
        }

        func testSessionAgentTranscriptIDsAreStableAndOccurrenceAware() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            let agentID = "agent:child-1"
            let repeated = MessageDto(role: "assistant", blocks: [.text(text: "same")])
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(agentId: agentID, name: "worker", agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent(agentID)

            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [repeated, repeated],
                nextMessageIndex: 2,
                revision: 1
            ))
            let firstIDs = source.model.selectedAgentMessages.map(\.id)
            XCTAssertEqual(firstIDs.count, 2)
            XCTAssertNotEqual(firstIDs[0], firstIDs[1])

            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [repeated, repeated],
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
                SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "worker",
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "idle",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent(agentID)

            let initial = MessageDto(role: "assistant", blocks: [.text(text: "visible")])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [initial],
                nextMessageIndex: 1,
                revision: 10
            ))
            XCTAssertEqual(source.model.selectedAgentMessages.count, 1)
            XCTAssertEqual(source.model.agentTranscripts[agentID]?.revision, 10)

            // Compaction can replace the content/details while retaining the
            // same number of visible rows. The raw revision is the only fence
            // that distinguishes this legitimate update from a stale reply.
            let compacted = MessageDto(role: "assistant", blocks: [
                .compactBoundary(messagesBefore: 4, messagesAfter: 1, summary: "fresh compact summary")
            ])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [compacted],
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
                SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "worker",
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "idle",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent(agentID)

            let fresh = MessageDto(role: "assistant", blocks: [.text(text: "fresh")])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [fresh],
                nextMessageIndex: 1,
                revision: 20
            ))

            let delayed = MessageDto(role: "assistant", blocks: [.text(text: "delayed stale")])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [delayed],
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
                SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "worker",
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "working",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent(agentID)

            let history = MessageDto(role: "assistant", blocks: [.text(text: "history")])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [history],
                nextMessageIndex: 1,
                revision: 10
            ))
            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                messageIndex: 1,
                message: MessageDto(role: "assistant", blocks: [.text(text: "live tail")])
            ))

            // A hidden/lifecycle record can advance the raw revision before
            // the live visible row. If that older snapshot is delivered late,
            // its lower visible watermark must not erase the indexed tail.
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [history],
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
                SessionAgentSummaryDto(agentId: agentID, name: "worker", agentType: "general", model: nil, modelProfile: nil, status: "working", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent(agentID)

            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                messageIndex: 0,
                message: MessageDto(role: "assistant", blocks: [
                    .toolUse(id: "tool-1", tool: "Read", inputJson: #"{"path":"a.txt"}"#, header: nil)
                ])
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
                messageIndex: 1,
                message: MessageDto(role: "user", blocks: [
                    .toolResult(id: "tool-1", tool: "Read", resultJson: #"{"content":"ok"}"#, isError: false, oldString: nil, newString: nil, filePath: nil, display: nil)
                ])
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
                SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "worker",
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "working",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent(agentID)

            let use = MessageDto(role: "assistant", blocks: [
                .toolUse(id: "tool-snapshot", tool: "Read", inputJson: #"{"path":"a.txt"}"#, header: nil)
            ])
            let result = MessageDto(role: "user", blocks: [
                .toolResult(id: "tool-snapshot", tool: "Read", resultJson: #"{"content":"ok"}"#, isError: false, oldString: nil, newString: nil, filePath: nil, display: nil)
            ])
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [use, result],
                nextMessageIndex: 2,
                revision: 1
            ))

            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: agentID,
                messageIndex: 1,
                message: use
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
                SessionAgentSummaryDto(agentId: agentID, name: "worker", agentType: "general", model: nil, modelProfile: nil, status: "working", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent(agentID)

            for (index, text) in ["first", "second"].enumerated() {
                source.applyForTesting(.sessionAgentMessage(
                    sessionId: "session-a",
                    agentId: agentID,
                    messageIndex: UInt64(index),
                    message: MessageDto(role: "assistant", blocks: [.text(text: text)])
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
                SessionAgentSummaryDto(agentId: first, name: "first", agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil),
                SessionAgentSummaryDto(agentId: second, name: "second", agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil)
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
                SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "worker",
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
                SessionAgentSummaryDto(agentId: agentID, name: "old", agentType: "general", model: nil, modelProfile: nil, status: "idle", latestActivity: nil, updatedAtMs: nil)
            ]))
            source.selectAgent(agentID)
            source.model.activeSessionId = "session-new"
            source.model.clearAgentState()

            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-old",
                agentId: agentID,
                messages: [MessageDto(role: "assistant", blocks: [.text(text: "stale")])],
                nextMessageIndex: 1,
                revision: 1
            ))
            XCTAssertTrue(source.model.agentTranscripts.isEmpty)
            XCTAssertEqual(source.model.selectedAgentID, ConversationModel.mainAgentID)
        }

        func testLiveAgentMessageMergesWithTranscriptReplyAndMarksAgentWorking() {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(
                    agentId: "agent:child-1",
                    name: "worker",
                    agentType: "general",
                    model: nil,
                    modelProfile: nil,
                    status: "idle",
                    latestActivity: nil,
                    updatedAtMs: nil
                )
            ]))
            source.selectAgent("agent:child-1")

            let live = MessageDto(role: "assistant", blocks: [.text(text: "live tail")])
            source.applyForTesting(.sessionAgentMessage(
                sessionId: "session-a",
                agentId: "agent:child-1",
                messageIndex: 0,
                message: live
            ))
            XCTAssertEqual(source.model.selectedAgentMessages.map(\.text), ["live tail"])
            XCTAssertEqual(source.model.selectedAgentSummary?.status, "working")

            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: "agent:child-1",
                messages: [
                    MessageDto(role: "assistant", blocks: [.text(text: "history")]),
                    live
                ],
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
            let same = MessageDto(role: "assistant", blocks: [.text(text: "same")])
            source.applyForTesting(.sessionAgentList(sessionId: "session-a", agents: [
                SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "worker",
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
                messageIndex: 1,
                message: same
            ))
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: "session-a",
                agentId: agentID,
                messages: [same],
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
            XCTAssertTrue(
                ConversationSessionRestorePolicy.shouldClearUnavailableSession(
                    unavailableSessionID: "saved-session",
                    pendingRestoreID: "saved-session",
                    activeSessionID: "saved-session"
                )
            )
            XCTAssertFalse(
                ConversationSessionRestorePolicy.shouldClearUnavailableSession(
                    unavailableSessionID: "old-session",
                    pendingRestoreID: "new-session",
                    activeSessionID: "new-session"
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

        func testMissingResumeCreatesReplacementWithoutShowingEngineError() async {
            let source = makeSource()
            let recorder = SessionTransitionRecorder(failure: .missing)
            source.setCommandSubmitterForTesting { command in
                try await recorder.submit(command)
            }

            source.resumeSession("missing-session")
            let receivedCommands = await recorder.waitForCommandCount(2)
            XCTAssertTrue(receivedCommands)

            let commands = await recorder.snapshot()
            XCTAssertEqual(commands, [.resume, .new])
            XCTAssertEqual(
                source.model.sessionRestoreRecovery?.unavailableSessionID,
                "missing-session"
            )
            XCTAssertTrue(source.model.sessionTransitionPending)
            XCTAssertTrue(source.model.isNew)
            XCTAssertNil(source.model.error)
            XCTAssertEqual(source.model.statusLine, "原会话已不存在，已创建新对话")

            source.applyForTesting(.sessionStarted(sessionId: "replacement-session"))
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertEqual(source.model.activeSessionId, "replacement-session")
            XCTAssertEqual(source.model.sessionRefreshRevision, 1)
        }

        func testNonMissingResumeFailureRemainsVisible() async {
            let source = makeSource()
            let recorder = SessionTransitionRecorder(failure: .generic)
            source.setCommandSubmitterForTesting { command in
                try await recorder.submit(command)
            }

            source.resumeSession("saved-session")
            let receivedCommand = await recorder.waitForCommandCount(1)
            XCTAssertTrue(receivedCommand)
            let transitionSettled = await waitForSessionTransitionToSettle(source)
            XCTAssertTrue(transitionSettled)

            let commands = await recorder.snapshot()
            XCTAssertEqual(commands, [.resume])
            XCTAssertNil(source.model.sessionRestoreRecovery)
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertNotNil(source.model.error)
            XCTAssertEqual(
                source.model.sessionTransitionFailure?.requestedSessionID,
                "saved-session"
            )
        }

        func testRejectedMissingResumeUsesTypedFallback() async {
            let source = makeSource()
            let recorder = SessionTransitionRecorder(failure: .missingRejected)
            source.setCommandSubmitterForTesting { command in
                try await recorder.submit(command)
            }

            source.resumeSession("missing-session")
            let receivedCommands = await recorder.waitForCommandCount(2)
            XCTAssertTrue(receivedCommands)

            let commands = await recorder.snapshot()
            XCTAssertEqual(commands, [.resume, .new])
            XCTAssertNil(source.model.error)
        }

        func testConfirmedEmptyResumePreservesSessionIDThroughDedicatedEntryPoint() async {
            let source = makeSource()
            let recorder = EmptySessionResumeRecorder()
            source.setCommandSubmitterForTesting { _ in
                XCTFail("a confirmed empty session must not use ResumeSession")
            }
            source.setEmptySessionResumerForTesting { sessionID, title in
                await recorder.resume(sessionID: sessionID, title: title)
            }

            source.resumeSession("empty-session", emptySessionTitle: "空会话")
            let didResume = await recorder.waitForResume()
            XCTAssertTrue(didResume)

            let request = await recorder.snapshot()
            XCTAssertEqual(
                request,
                .init(sessionID: "empty-session", title: "空会话")
            )
            XCTAssertTrue(source.model.sessionTransitionPending)

            source.applyForTesting(.sessionResumed(sessionId: "empty-session", messages: []))
            XCTAssertEqual(source.model.activeSessionId, "empty-session")
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertEqual(source.model.sessionRefreshRevision, 1)
        }

        func testBootstrapSessionStartedDoesNotClearPendingResume() {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in }

            source.resumeSession("saved-session")
            XCTAssertTrue(source.model.sessionTransitionPending)

            source.applyForTesting(.sessionStarted(sessionId: "startup-session"))
            XCTAssertTrue(
                source.model.sessionTransitionPending,
                "a delayed bootstrap SessionStarted is not ResumeSession confirmation"
            )

            source.applyForTesting(.sessionResumed(sessionId: "saved-session", messages: []))
            XCTAssertFalse(source.model.sessionTransitionPending)
            XCTAssertEqual(source.model.activeSessionId, "saved-session")
        }

        func testSameTargetSessionStartedCannotCompleteResumeBeforeTranscriptReplay() {
            let source = makeSource()
            source.setCommandSubmitterForTesting { _ in }
            source.model.activeSessionId = "saved-session"

            source.resumeSession("saved-session")
            XCTAssertTrue(source.model.sessionTransitionPending)

            source.applyForTesting(.sessionStarted(sessionId: "saved-session"))

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
                SessionAgentSummaryDto(
                    agentId: agentID,
                    name: "design",
                    agentType: "designer",
                    model: "deepseek-v4-flash",
                    modelProfile: "deepseek",
                    status: "running",
                    latestActivity: "Inspecting layout",
                    updatedAtMs: 10
                )
            ]))
            source.applyForTesting(.sessionAgentTranscript(
                sessionId: sessionID,
                agentId: agentID,
                messages: [MessageDto(role: "assistant", blocks: [.text(text: "Design notes")])],
                nextMessageIndex: 1,
                revision: 1
            ))
            source.selectAgent(agentID)

            source.applyForTesting(.sessionStarted(sessionId: sessionID))

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
            source.model.activeSessionId = "session-a"

            source.resumeSession("session-b")
            source.applyForTesting(.sessionResumed(
                sessionId: "session-b",
                messages: [MessageDto(role: "assistant", blocks: [.text(text: "B")])]
            ))
            source.resumeSession("session-c")
            source.applyForTesting(.sessionResumed(
                sessionId: "session-c",
                messages: [MessageDto(role: "assistant", blocks: [.text(text: "C")])]
            ))

            source.applyForTesting(.sessionResumed(
                sessionId: "session-b",
                messages: [MessageDto(role: "assistant", blocks: [.text(text: "late B")])]
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

            source.applyForTesting(.sessionStarted(sessionId: "bootstrap-session"))

            XCTAssertEqual(source.model.messages, [visibleMessage])
            XCTAssertEqual(source.model.items, [.message(visibleMessage)])
            XCTAssertEqual(source.model.activeSessionId, "active-session")
            XCTAssertFalse(source.model.isNew)
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
                    path: "/tmp/retained.jsonl"
                ),
            ]))

            XCTAssertEqual(sessionIDsObservedWhenLoaded, [uuid()])
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
                MessageDto(role: "user", blocks: [.text(text: "第一条用户消息")]),
                MessageDto(role: "assistant", blocks: [.text(text: "助手的回复")]),
            ]
            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: messages))

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
            let assistant = MessageDto(role: "assistant", blocks: [
                .text(text: "正文"),
                .thinking(thinking: "推理", signature: nil),
                .compactBoundary(messagesBefore: 8, messagesAfter: 2,
                                 summary: "hidden compact summary"),
                .toolUse(id: "t1", tool: "Read", inputJson: "{\"path\":\"a\"}", header: nil),
                .toolResult(id: "t1", tool: "", resultJson: "\"ok\"",
                            isError: false, oldString: nil, newString: nil, filePath: nil,
                            display: nil),
            ])
            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: [assistant]))

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
                MessageDto(role: "user", blocks: [.text(text: "执行检查")]),
                MessageDto(role: "assistant", blocks: [
                    .text(text: "开始"),
                    .toolUse(
                        id: "tool-restore",
                        tool: "Read",
                        inputJson: #"{"file_path":"a.rs"}"#,
                        header: nil),
                ]),
                MessageDto(role: "user", blocks: [
                    .toolResult(
                        id: "tool-restore",
                        tool: "Read",
                        resultJson: #"{"result":"ok"}"#,
                        isError: false,
                        oldString: nil,
                        newString: nil,
                        filePath: nil,
                        display: nil),
                ]),
                MessageDto(role: "assistant", blocks: [.text(text: "检查完成")]),
            ]

            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: messages))

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
        }

        func testSessionResumedPreservesNarrativeToolInterleavingWithinOneAssistantMessage() {
            let source = makeSource()
            let messages: [MessageDto] = [
                MessageDto(role: "assistant", blocks: [
                    .text(text: "A"),
                    .toolUse(id: "tool-1", tool: "Read", inputJson: #"{"path":"a"}"#, header: nil),
                    .toolResult(id: "tool-1", tool: "Read", resultJson: #"{"result":"a"}"#, isError: false, oldString: nil, newString: nil, filePath: nil, display: nil),
                    .text(text: "B"),
                    .toolUse(id: "tool-2", tool: "Edit", inputJson: #"{"path":"b"}"#, header: nil),
                    .toolResult(id: "tool-2", tool: "Edit", resultJson: #"{"result":"b"}"#, isError: false, oldString: nil, newString: nil, filePath: nil, display: nil),
                    .text(text: "C"),
                ]),
            ]

            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: messages))

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
                MessageDto(role: "user", blocks: [.text(text: "继续尝试")]),
                MessageDto(role: "assistant", blocks: [
                    .toolUse(
                        id: "cancelled-tool",
                        tool: "Shell",
                        inputJson: #"{"command":"long job"}"#,
                        header: nil),
                ]),
                MessageDto(role: "user", blocks: [
                    .toolResult(
                        id: "cancelled-tool",
                        tool: "Shell",
                        resultJson: #"{"error":"interrupted","tool_denial_kind":"interrupted"}"#,
                        isError: true,
                        oldString: nil,
                        newString: nil,
                        filePath: nil,
                        display: nil),
                ]),
                MessageDto(role: "assistant", blocks: [.text(text: "已改用其他方法")]),
            ]

            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: messages))

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
            let assistant = MessageDto(role: "assistant", blocks: [
                .text(text: "我来读一下"),
                .toolUse(id: "t9", tool: "Read", inputJson: "{\"file_path\":\"a.rs\"}", header: nil),
            ])
            let toolTurn = MessageDto(role: "user", blocks: [
                .toolResult(id: "t9", tool: "Read", resultJson: "{\"result\":\"ok\"}",
                            isError: false, oldString: nil, newString: nil, filePath: nil,
                            display: nil),
            ])
            let followUp = MessageDto(role: "user", blocks: [.text(text: "继续")])

            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(
                .sessionResumed(sessionId: uuid(), messages: [assistant, toolTurn, followUp]))

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
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: []))

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
            let system = MessageDto(role: "system", blocks: [.text(text: "系统提示")])
            source.expectSessionResumeForTesting(uuid())
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: [system]))

            XCTAssertEqual(source.model.messages.count, 1)
            XCTAssertEqual(source.model.messages[0].role, .ai)
            XCTAssertEqual(source.model.messages[0].text, "系统提示")
        }

        func testConcurrentEngineEntryPointsShareOneHandleBuild() async throws {
            let gate = EngineSubmitGate()
            let handle = TestMobileEngineHandle { command in
                try await gate.submit(command)
            }
            var buildCount = 0
            let source = makeSource(handleBuilder: { _, _, _ in
                buildCount += 1
                return handle
            })

            source.warmUp()
            source.listSessions()
            source.send("并发消息")
            let prepare = Task { try await source.prepare() }

            await gate.waitForFirstSubmit()
            XCTAssertEqual(buildCount, 1)
            await gate.releaseFirstSubmit()
            try await prepare.value
            let receivedCommands = await gate.waitForSubmitCount(4)
            XCTAssertTrue(receivedCommands)

            XCTAssertEqual(buildCount, 1,
                           "warm-up, prepare, list and send must await the same engine build")
            let commands = await gate.snapshot()
            var sessionListCommandCount = 0
            for command in commands {
                guard case let .listSessions(limit) = command else { continue }
                sessionListCommandCount += 1
                XCTAssertEqual(limit, UInt32.max,
                               "bootstrap and drawer refresh must both request the full catalog")
            }
            XCTAssertGreaterThan(sessionListCommandCount, 0)
        }

        func testBootstrapRequestsTheCompleteSessionCatalog() async throws {
            let gate = EngineSubmitGate()
            let handle = TestMobileEngineHandle { command in
                try await gate.submit(command)
            }
            let source = makeSource(handleBuilder: { _, _, _ in handle })

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
            guard case let .listSessions(limit) = sessionList else { return }
            XCTAssertEqual(limit, UInt32.max,
                           "project persistence requires an uncapped catalog")
        }

        func testFailedSharedEngineBuildCanRetryWithoutCachingPartialHandle() async throws {
            let failingGate = EngineSubmitGate(firstSubmitError: EngineBuildTestError.bootstrapFailed)
            let succeedingGate = EngineSubmitGate()
            var buildCount = 0
            let source = makeSource(handleBuilder: { _, _, _ in
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

        private func makeSource(
            handleBuilder: @escaping EngineConversationSource.HandleBuilder
        ) -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory(),
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
        struct Request: Equatable {
            let sessionID: String
            let title: String
        }

        private var request: Request?

        func resume(sessionID: String, title: String) {
            request = Request(sessionID: sessionID, title: title)
        }

        func waitForResume() async -> Bool {
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(1))
            while request == nil, clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(1))
            }
            return request != nil
        }

        func snapshot() -> Request? {
            request
        }
    }

    private final class TestMobileEngineHandle: MobileEngineHandle {
        private let submitHandler: (ClientCommand) async throws -> Void

        required init(unsafeFromRawPointer pointer: UnsafeMutableRawPointer) {
            submitHandler = { _ in }
            super.init(unsafeFromRawPointer: pointer)
        }

        init(submitHandler: @escaping (ClientCommand) async throws -> Void) {
            self.submitHandler = submitHandler
            super.init(noPointer: .init())
        }

        override func submit(command: ClientCommand) async throws {
            try await submitHandler(command)
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

#endif
