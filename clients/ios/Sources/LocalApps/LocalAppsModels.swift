import Foundation

enum LocalAppsDistributionMode: String, Sendable {
    case store
    case full

    static var current: Self {
        let value = Bundle.main.object(forInfoDictionaryKey: "LingxiLocalAppsMode") as? String
        if let value, let mode = Self(rawValue: value.lowercased()) { return mode }
        return Bundle.main.bundleIdentifier?.hasSuffix(".full") == true ? .full : .store
    }

    var runtimeLabel: String {
        switch self {
        case .store: String(localized: "local_apps_runtime_static")
        case .full: String(localized: "local_apps_runtime_vite")
        }
    }
}

/// Generic per-app glyph. Every app starts from a brief, so there is no fixed
/// category taxonomy to key an icon off of.
let localAppIconSystemName = "app.badge"

struct LocalAppSummary: Identifiable, Hashable, Sendable {
    let id: String
    var name: String
    /// One-line description the user gave at creation time.
    var brief: String
    var gitEnabled: Bool = true
    var updatedAt: Date
    var workflow: LocalAppWorkflow
    var workspaceRelativePath: String
    /// The app's pinned "init" session (bare uuid), listed first in its
    /// session catalog. `nil` for pre-v3 records before the boot backfill.
    var initSessionId: String? = nil
}

enum LocalAppLaunchDestination: String, Hashable, Sendable {
    case details
    case preview
}

/// Workflow state of an app — the v3 two-state model (protocol 5.0.0's
/// `AppWorkflowStateDto`). The old 14-state designer/plan/generation pipeline
/// is gone: apps are agent-driven conversations now, so an app is either a
/// draft or ready.
enum LocalAppWorkflow: String, CaseIterable, Hashable, Sendable {
    case draft
    case ready

    var label: String {
        switch self {
        case .draft: String(localized: "local_apps_state_draft")
        case .ready: String(localized: "local_apps_state_ready")
        }
    }
}

/// Which shape an app draws — the UI mirror of protocol 7.0.0's
/// `AppSurfaceDto`. Chosen in the create sheet and IMMUTABLE afterwards: the
/// scaffold is laid down at creation, so this is the one field the sheet must
/// get in front of the user rather than infer silently.
enum LocalAppSurface: String, CaseIterable, Hashable, Sendable {
    case dom
    case canvas

    var label: String {
        switch self {
        case .dom: String(localized: "local_apps_surface_dom")
        case .canvas: String(localized: "local_apps_surface_canvas")
        }
    }

    /// One line under the picker saying what the choice actually decides —
    /// "canvas" is a build detail, "一块自己绘制的画面" is something a user can
    /// tell is right or wrong.
    var detail: String {
        switch self {
        case .dom: String(localized: "local_apps_surface_dom_detail")
        case .canvas: String(localized: "local_apps_surface_canvas_detail")
        }
    }
}

/// One row of an app's workspace-scoped session catalog — the UI projection of
/// the wire `AppSessionRowDto`. `isInit` marks the pinned init session
/// (`AppSessionKindDto.init`), listed first regardless of modified order.
struct LocalAppSessionRow: Identifiable, Hashable, Sendable {
    /// Bare session uuid — also the resume key.
    let uuid: String
    let title: String
    /// Relative display form of the wire's RFC 3339 `modified` stamp.
    let relativeTime: String
    let messageCount: Int
    let isInit: Bool

    var id: String { uuid }
}

/// One requested page of an app's session catalog plus the paging cursor
/// (`nextOffset == nil` on the last page).
struct LocalAppSessionPage: Hashable, Sendable {
    var rows: [LocalAppSessionRow] = []
    var nextOffset: UInt64?
}

struct LocalAppProfileProposal: Identifiable, Equatable, Sendable {
    let appID: String
    let approvalToken: String
    let baseRevision: UInt64
    let currentRevision: UInt64
    let instructions: String
    let reason: String

    var id: String { approvalToken }
}

enum LocalAppDataFieldType: String, CaseIterable, Hashable, Sendable {
    case text
    case longText = "long_text"
    case integer
    case decimal
    case boolean
    case dateTime = "date_time"
    case enumeration = "enum"
    case imageReference = "image_ref"
}

struct LocalAppDataField: Identifiable, Hashable, Sendable {
    var id: String
    var label: String
    var fieldType: LocalAppDataFieldType
    var required: Bool
    var options: [String]
}

struct LocalAppDataCollection: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let fields: [LocalAppDataField]
    let enabledByDefault: Bool
}

enum LocalAppRuntimeStatus: Hashable, Sendable {
    case stopped
    case starting
    case running(URL?)
    case suspended(String?)
    case stopping
    case failed(String)

    var label: String {
        switch self {
        case .stopped: String(localized: "local_apps_runtime_stopped")
        case .starting: String(localized: "local_apps_runtime_starting")
        case .running: String(localized: "local_apps_runtime_running")
        case let .suspended(reason): reason.map { String(localized: "local_apps_runtime_suspended \($0)") } ?? String(localized: "local_apps_runtime_suspended_plain")
        case .stopping: String(localized: "local_apps_runtime_stopping")
        case let .failed(reason): String(localized: "local_apps_runtime_failed \(reason)")
        }
    }

    var url: URL? {
        guard case let .running(url) = self else { return nil }
        return url
    }
}

/// What the pushed preview route shows when there is no live URL to load.
///
/// Extracted from the view so the mapping is testable. The route used to
/// collapse `.starting`, `.stopped` and `.failed(reason)` into a single
/// hourglass with no action and no reason, which made a runtime that had
/// FAILED indistinguishable from one that was merely still booting — and left
/// the user no way to retry.
enum LocalAppPreviewPlaceholder: Equatable, Sendable {
    /// A transition is in flight; the URL should arrive on its own.
    case transient
    /// The runtime failed and will not come up without action.
    case failed(String)
    /// The host suspended the runtime.
    case suspended(String?)
    /// Nothing is running and nothing is in flight.
    case idle

    /// `nil` means "there is a URL — render the web view instead".
    static func forStatus(_ status: LocalAppRuntimeStatus?) -> LocalAppPreviewPlaceholder? {
        switch status {
        case let .running(url):
            // `running` without a URL yet is still coming up, not idle.
            url == nil ? .transient : nil
        case .starting, .stopping:
            .transient
        case let .failed(reason):
            .failed(reason)
        case let .suspended(reason):
            .suspended(reason)
        case .stopped, nil:
            .idle
        }
    }
}

struct LocalAppCheckpoint: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let kind: String
    let createdAt: Date
}

enum LocalAppCapabilityDecision: String, CaseIterable, Identifiable, Sendable {
    case once
    case session
    case always
    case deny

    var id: String { rawValue }

    var label: String {
        switch self {
        case .once: String(localized: "local_apps_allow_once")
        case .session: String(localized: "local_apps_allow_session_short")
        case .always: String(localized: "local_apps_allow_always")
        case .deny: String(localized: "local_apps_deny")
        }
    }
}

struct LocalAppPermissionPrompt: Identifiable, Hashable, Sendable {
    enum Kind: Hashable, Sendable {
        case dataMutation
        case uiControl
        case networkDomain
        case restoreCheckpoint
        case camera
        case photoLibrary
        case microphone
        case location
        case notifications
        case clipboard
        case share
        case textToSpeech
        case files
        case filesRead
        case filesWrite
        case deviceStatus
        case haptics
        case deepLink
        case calendar
        case contacts
        case media
        case llm
        case agentNotify
        case backgroundSchedule
        case uiAction(String)
    }

    let id: String
    let appID: String
    let kind: Kind
    let reason: String
    let domain: String?

    var title: String {
        switch kind {
        case .dataMutation: String(localized: "local_apps_permission_data_mutation")
        case .uiControl: String(localized: "local_apps_permission_ui_control")
        case .networkDomain: String(localized: "local_apps_permission_network")
        case .restoreCheckpoint: String(localized: "local_apps_permission_restore")
        case .camera: String(localized: "local_apps_permission_camera")
        case .photoLibrary: String(localized: "local_apps_permission_photo_library")
        case .microphone: String(localized: "local_apps_permission_microphone")
        case .location: String(localized: "local_apps_permission_location")
        case .notifications: String(localized: "local_apps_permission_notifications")
        case .clipboard: "允许应用读取或写入系统剪贴板？"
        case .share: "允许应用打开系统分享面板？"
        case .textToSpeech: "允许应用将文字转换为语音？"
        case .filesRead:
            "允许应用读取自己的私有文件？"
        case .filesWrite:
            "允许应用写入自己的私有文件？"
        case .files:
            if reason.contains("写入") {
                "允许应用写入自己的私有文件？"
            } else if reason.contains("读取") {
                "允许应用读取自己的私有文件？"
            } else {
                "允许应用访问自己的私有文件？"
            }
        case .deviceStatus: "允许应用读取设备状态？"
        case .haptics: "允许应用触发触觉反馈？"
        case .deepLink: "允许应用打开外部链接？"
        case .calendar: "允许应用读取指定范围内的日历事件？"
        case .contacts: "允许应用搜索联系人？"
        case .media: "允许应用读取自己刚获取的媒体？"
        case .llm: String(localized: "local_apps_permission_llm")
        case .agentNotify: String(localized: "local_apps_permission_agent_notify")
        case .backgroundSchedule: "允许应用在系统后台按计划运行流程？"
        case let .uiAction(action): String(localized: "local_apps_permission_ui_action \(action)")
        }
    }
}
