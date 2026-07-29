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
                appSandboxRoot: NSTemporaryDirectory())
            return EngineConversationSource(config: config)
        }

        private func uuid() -> String { "22222222-2222-4222-8222-222222222222" }

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

        /// An assistant message carrying multiple block kinds (text + thinking +
        /// tool_use + tool_result) must flatten into the single display string the
        /// iOS `Message` model carries — text/thinking bodies plus a labeled line
        /// for the tool blocks (the conversation surface has no tool cards yet), so
        /// no restored content silently vanishes.
        func testSessionResumedFlattensRichAssistantBlocks() {
            let source = makeSource()
            let assistant = MessageDto(role: "assistant", blocks: [
                .text(text: "正文"),
                .thinking(thinking: "推理", signature: nil),
                .compactBoundary(messagesBefore: 8, messagesAfter: 2,
                                 summary: "hidden compact summary"),
                .toolUse(id: "t1", tool: "Read", inputJson: "{\"path\":\"a\"}"),
                .toolResult(id: "t1", tool: "", resultJson: "\"ok\"",
                            isError: false, oldString: nil, newString: nil, filePath: nil),
            ])
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: [assistant]))

            XCTAssertEqual(source.model.messages.count, 1)
            let text = source.model.messages[0].text
            XCTAssertTrue(text.contains("正文"), "text block body must be present")
            XCTAssertTrue(text.contains("推理"), "thinking block body must be present")
            XCTAssertTrue(text.contains("对话已压缩"),
                          "compact boundary must remain visible after resume")
            XCTAssertFalse(text.contains("hidden compact summary"),
                           "internal compact summary must not be rendered as user text")
            XCTAssertTrue(text.contains("调用工具 Read"),
                          "a tool_use block must surface a labeled line, not vanish")
            XCTAssertEqual(source.model.messages[0].role, .ai,
                           "assistant role maps to the AI side")
        }

        /// A zero-message resume (a session with no transcript) must still adopt
        /// the id, clear the placeholder, and fall into the empty-state — never
        /// leave a stale placeholder bubble behind.
        func testSessionResumedEmptyTranscriptClearsToEmptyState() {
            let source = makeSource()
            source.model.messages = [Message(role: .ai, text: "placeholder")]

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
            source.applyForTesting(.sessionResumed(sessionId: uuid(), messages: [system]))

            XCTAssertEqual(source.model.messages.count, 1)
            XCTAssertEqual(source.model.messages[0].role, .ai)
            XCTAssertEqual(source.model.messages[0].text, "系统提示")
        }
    }

#endif
