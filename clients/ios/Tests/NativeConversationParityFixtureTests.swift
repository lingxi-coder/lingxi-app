import Foundation
import XCTest
@testable import LingxiCode
#if canImport(engine_mobileFFI)
import engine_mobileFFI
#endif

/// Shared projection snapshots and real permission/workflow reducer inputs, without engine startup.
@MainActor
final class NativeConversationParityFixtureTests: XCTestCase {
    private func scenarios() throws -> [[String: Any]] {
        let clients = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let file = clients.appendingPathComponent("shared/fixtures/native-conversation-parity.json")
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        XCTAssertEqual(fixture["version"] as? Int, 1)
        return try XCTUnwrap(fixture["scenarios"] as? [[String: Any]])
    }

    func testSharedToolSnapshotsPreserveGroupingStatusesAndActiveSelection() throws {
        for scenario in try scenarios() {
            let events = try XCTUnwrap(scenario["events"] as? [[String: Any]])
            let groups = try events.map { event -> ConversationTimelineGroup in
                let id = try XCTUnwrap(event["id"] as? String)
                if event["kind"] as? String == "reasoning" {
                    return ConversationTimelineGroup(id: id, runID: "fixture-run", rows: [
                        .reasoning(runID: "fixture-run", activityID: id, text: event["text"] as! String)
                    ], status: .completed)
                }
                let status: ConversationToolStatus
                switch event["status"] as? String {
                case "running": status = .running
                case "completed": status = .completed
                case "failed": status = .failed
                case "cancelled": status = .cancelled
                default: throw NSError(domain: "Unknown fixture status", code: 1)
                }
                return ConversationTimelineGroup(id: id, runID: "fixture-run", rows: [
                    .tool(runID: "fixture-run", trace: ConversationToolTrace(id: id, tool: event["name"] as! String, status: status))
                ], status: status == .running ? .running : .completed)
            }
            let projected = ConversationDesktopTimeline.groups(groups)
            let expected = try XCTUnwrap(scenario["expected_groups"] as? [[String: Any]])
            XCTAssertEqual(projected.count, expected.count, scenario["id"] as! String)
            for (group, want) in zip(projected, expected) {
                let tools = group.rows.compactMap { row -> ConversationToolTrace? in
                    if case let .tool(_, trace) = row { return trace }; return nil
                }
                XCTAssertEqual(tools.map(\.id), want["ids"] as? [String])
                let statuses = tools.map { trace in
                    switch trace.status {
                    case .running: return "running"
                    case .completed: return "completed"
                    case .failed: return "failed"
                    case .cancelled: return "cancelled"
                    case .unknown: return "unknown"
                    }
                }
                XCTAssertEqual(statuses, want["statuses"] as? [String])
                XCTAssertEqual(ConversationDesktopTimeline.activeTools(tools).map(\.id), want["active_ids"] as? [String])
                XCTAssertEqual(ConversationDesktopTimeline.summary(tools), want["summary"] as? String)
                XCTAssertEqual(group.rows.count, tools.count, "Historical reasoning must stay hidden")
            }
        }
    }

    #if canImport(engine_mobileFFI)
    func testSharedPermissionRetainsCorrelatorAndSuppressedRule() throws {
        for scenario in try scenarios() {
            guard let row = scenario["permission"] as? [String: Any] else { continue }
            let pending = PendingPermission(request: PermissionRequest(
                requestId: UInt64(row["request_id"] as! Int),
                kind: .toolUseConfirm(toolName: row["tool"] as! String, toolInputJson: row["input_json"] as! String, defaultAllow: false),
                worker: nil, owner: nil, suppressAlwaysAllowRule: row["suppress_always_allow"] as! Bool, autoModePrompt: nil))
            XCTAssertEqual(pending.requestId, UInt64(row["request_id"] as! Int))
            XCTAssertEqual(pending.suppressAlwaysAllowRule, row["suppress_always_allow"] as? Bool)
        }
    }

    func testSharedWorkflowRejectsLateProgressAndForeignSessions() throws {
        struct Offline: Error {}
        for scenario in try scenarios() {
            guard let updates = scenario["workflow_updates"] as? [[String: Any]] else { continue }
            let source = EngineConversationSource(config: EngineConfig(
                apiBase: "https://invalid.example", apiKey: "", model: "", appSandboxRoot: NSTemporaryDirectory(),
                projectCwd: nil, sessionMode: .code, visionDelegationEnabled: false),
                handleBuilder: { _, _, _ in throw Offline() })
            source.model.activeSessionId = scenario["active_session"] as! String
            for row in updates {
                source.applyWorkflowProgressForTesting(originSessionId: row["origin"] as! String,
                    taskId: "fixture-task", runId: row["run"] as! String,
                    progress: ConversationWorkflowProgressPayload(kind: .workflowAgent, index: 0, title: "Fixture agent",
                        message: row["message"] as? String, label: nil, phaseIndex: 0, phaseTitle: "Test",
                        agentId: "fixture-agent", agentType: "test", model: nil, fallbackModel: nil,
                        state: ConversationWorkflowAgentState(rawValue: row["state"] as! String), error: nil,
                        toolUseId: nil, startedAtMs: 2, queuedAtMs: 1, lastProgressAtMs: UInt64(row["time"] as! Int),
                        attempt: 1, lastAttemptReason: nil, tokens: nil, toolCalls: nil, lastToolName: nil,
                        lastToolSummary: nil, promptPreview: nil))
            }
            let expected = try XCTUnwrap(scenario["expected_workflow"] as? [String: String])
            let run = try XCTUnwrap(source.model.backgroundTasks.first(where: { $0.id == "fixture-task" })?.workflow)
            XCTAssertEqual(run.runId, expected["run"])
            XCTAssertEqual(run.agents.first?.message, expected["message"])
            XCTAssertEqual(run.agents.first?.state.rawValue, expected["state"])
        }
    }
    #endif
}
