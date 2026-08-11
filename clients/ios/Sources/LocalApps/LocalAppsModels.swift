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
        case .full: String(localized: "local_apps_runtime_next")
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
        case llm
        case agentNotify
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
        case .llm: String(localized: "local_apps_permission_llm")
        case .agentNotify: String(localized: "local_apps_permission_agent_notify")
        case let .uiAction(action): String(localized: "local_apps_permission_ui_action \(action)")
        }
    }
}
