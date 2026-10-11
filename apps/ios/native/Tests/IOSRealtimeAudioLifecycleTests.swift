import XCTest
@testable import LingxiCode

final class IOSRealtimeAudioLifecycleTests: XCTestCase {
    func testCallbackMailboxPreservesOrderedEvents() async throws {
        let listener = IOSRealtimeAudioListenerAdapter()
        await listener.onEvent(eventJson: #"{"type":"session_ready"}"#)
        await listener.onEvent(eventJson: #"{"type":"transcript","text":"hello"}"#)
        listener.finish()
        var values: [String] = []
        for try await event in listener.events { values.append(event) }
        XCTAssertEqual(values, [#"{"type":"session_ready"}"#, #"{"type":"transcript","text":"hello"}"#])
    }

    func testCallbackOverflowTerminatesInsteadOfSilentlyDroppingAudio() async {
        let listener = IOSRealtimeAudioListenerAdapter()
        for index in 0 ..< 17 { await listener.onEvent(eventJson: "event-\(index)") }
        do {
            for try await _ in listener.events {}
            XCTFail("overflow must report a failure to the owning conversation")
        } catch {
            guard case .mediaTooLarge = error as? AudioServiceFailure else {
                return XCTFail("expected a bounded-media overflow failure")
            }
        }
    }

    func testOversizedCallbackFailsBeforeQueueingPayload() async {
        let listener = IOSRealtimeAudioListenerAdapter()
        await listener.onEvent(eventJson: String(repeating: "x", count: 1_500_001))
        do {
            for try await _ in listener.events { XCTFail("oversized input must not be queued") }
            XCTFail("oversized callback must fail")
        } catch {
            guard case .mediaTooLarge = error as? AudioServiceFailure else {
                return XCTFail("expected a bounded-media overflow failure")
            }
        }
    }
}
