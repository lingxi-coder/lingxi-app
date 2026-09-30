import XCTest
@testable import LingxiCode

@MainActor
final class TerminalLinuxRuntimeTaskOperationsTests: XCTestCase {
    func testRefreshTasksUpdatesMessageAndTasks() async {
        let model = LinuxRuntimeTaskOperationsModel(
            adapter: FakeLinuxRuntimeTaskOperator(
                refreshResult: .init(
                    tasks: [LinuxRuntimeTaskRow(id: "1", title: "shell", state: .running, detail: "alive")],
                    message: "已刷新 1 个 guest 任务"
                )
            )
        )

        let result = await model.refreshTasks(mode: .mobileLinux)

        XCTAssertEqual(result?.tasks.count, 1)
        XCTAssertEqual(result?.message, "已刷新 1 个 guest 任务")
        XCTAssertNil(model.busyOperation)
        XCTAssertNil(model.errorMessage)
    }

    func testStopTaskPropagatesFailureMessage() async {
        let model = LinuxRuntimeTaskOperationsModel(
            adapter: FakeLinuxRuntimeTaskOperator(
                stopError: LinuxRuntimeTaskOperationFailure(message: "停止失败")
            )
        )

        let result = await model.stopTask(mode: .mobileLinux, taskID: "task-1")

        XCTAssertNil(result)
        XCTAssertEqual(model.errorMessage, "停止失败")
        XCTAssertNil(model.busyOperation)
    }
}

private struct FakeLinuxRuntimeTaskOperator: LinuxRuntimeTaskOperating {
    var refreshResult: LinuxRuntimeTaskOperationResult = .init(tasks: [], message: "当前没有 guest 后台任务")
    var refreshError: LinuxRuntimeTaskOperationFailure?
    var stopResult: LinuxRuntimeTaskOperationResult = .init(tasks: [], message: "已停止任务")
    var stopError: LinuxRuntimeTaskOperationFailure?

    func refreshTasks(mode: LinuxRuntimeMode) async throws -> LinuxRuntimeTaskOperationResult {
        if let refreshError { throw refreshError }
        return refreshResult
    }

    func stopTask(mode: LinuxRuntimeMode, taskID: String) async throws -> LinuxRuntimeTaskOperationResult {
        if let stopError { throw stopError }
        return stopResult
    }
}
