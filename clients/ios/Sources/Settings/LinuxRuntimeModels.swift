import Foundation

enum LinuxRuntimeTaskState: Equatable {
    case running, completed, failed, cancelled, unavailable

    var label: String {
        switch self {
        case .running: return "运行中"
        case .completed: return "完成"
        case .failed: return "失败"
        case .cancelled: return "已停止"
        case .unavailable: return "不可用"
        }
    }
}

struct LinuxRuntimeTaskRow: Identifiable, Equatable {
    let id: String
    var title: String
    var state: LinuxRuntimeTaskState
    var detail: String?
}

struct LinuxRuntimeMountRow: Identifiable, Equatable {
    let id: String
    var hostPath: String
    var guestPath: String
    var readOnly: Bool
}

struct LinuxTerminalLine: Identifiable, Equatable {
    let id: UUID = UUID()
    var streamID: String
    var source: String
    var text: String
    var isError: Bool
}

struct LinuxTerminalState: Equatable {
    var draftCommand: String = "python3 --version"
    var activeSessionID: String? = nil
    var lines: [LinuxTerminalLine] = []
    var lastExitCode: Int32? = nil
    var lastError: String? = nil
}
