import XCTest
@testable import LingxiCode

final class CompactionProgressTests: XCTestCase {
    func testHybridProgressFixture() throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let url = root.appendingPathComponent("lingxi-code/client-protocol/snapshots/compaction_hybrid_progress.json")
        let fixture = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        for sample in fixture["cases"] as! [[String: Any]] {
            let milliseconds = (sample["elapsed_ms"] as! NSNumber).doubleValue
            XCTAssertEqual(ConversationCompactionProgress.percent(phase: sample["phase"] as! String, elapsed: milliseconds / 1000), (sample["percent"] as? NSNumber)?.intValue, "\(milliseconds)ms")
        }
    }

    func testEngineStagesOwnTheClockAndTerminalState() {
        let start = Date(timeIntervalSince1970: 100)
        var status = ConversationCompactionStatus.reducing(nil, phase: "preparing", error: nil, now: start)
        XCTAssertEqual(status, .running(phase: "preparing", startedAt: start, phaseStartedAt: start))
        status = .reducing(status, phase: "summarizing", error: nil, now: start)
        for phase in ["preparing", "summarizing"] {
            status = .reducing(status, phase: phase, error: nil, now: start.addingTimeInterval(90))
            XCTAssertEqual(status, .running(phase: "summarizing", startedAt: start, phaseStartedAt: start))
        }
        status = .reducing(status, phase: "restoring", error: nil, now: start.addingTimeInterval(90))
        XCTAssertEqual(status, .running(phase: "restoring", startedAt: start, phaseStartedAt: start.addingTimeInterval(90)))
        XCTAssertEqual(ConversationCompactionStatus.reducing(status, phase: "preparing", error: nil), status)
        let unknown = ConversationCompactionStatus.reducing(status, phase: "future_phase", error: nil)
        XCTAssertEqual(unknown, .running(phase: "restoring", startedAt: start, phaseStartedAt: start.addingTimeInterval(90), unknownPhase: true))
        XCTAssertEqual(ConversationCompactionStatus.reducing(unknown, phase: "preparing", error: nil), status)
        XCTAssertEqual(ConversationCompactionStatus.reducing(status, phase: "skipped", error: nil), .skipped)
        XCTAssertEqual(ConversationCompactionStatus.reducing(.skipped, phase: "complete", error: nil), .skipped)
        XCTAssertEqual(ConversationCompactionStatus.reducing(.skipped, phase: "preparing", error: nil, now: start), .running(phase: "preparing", startedAt: start, phaseStartedAt: start))
        XCTAssertNil(ConversationCompactionStatus.reducing(status, phase: "cancelled", error: nil))
        XCTAssertEqual(ConversationCompactionStatus.reducing(status, phase: "error", error: "API failed"), .failed(detail: "API failed"))
        let done = ConversationCompactionStatus.completed(messagesBefore: 42, messagesAfter: 8, bytesSaved: 2048)
        XCTAssertEqual(ConversationCompactionStatus.reducing(done, phase: "complete", error: nil), done)
    }
}
