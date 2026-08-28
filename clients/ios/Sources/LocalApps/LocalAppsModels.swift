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

/// The first message the conversational create flow sends on the user's behalf,
/// once the new shell's init session is live.
///
/// Named here rather than written as a literal at the call site so a test can
/// assert the exact string that gets SENT. Nothing in Swift fails to compile
/// over a missing localization key — this branch shipped a call site reading
/// `local_apps_kickoff_message`, a key in no catalog, which would have sent the
/// raw key as the user's first message — so the only way this can be checked at
/// all is for the value to be reachable from a test.
enum LocalAppKickoff {
    /// The catalog key. Shared with Android (`R.string.local_apps_kickoff`), so
    /// the copy is one sentence maintained once for both clients.
    ///
    /// A plain `String` rather than a `String.LocalizationValue` literal so the
    /// test can compare the RESOLVED copy against the key that produced it. A
    /// test comparing against a hardcoded `"local_apps_kickoff"` passes for a
    /// mistyped key — it was written that way first, and a run with the key
    /// deliberately broken passed.
    static let key = "local_apps_kickoff"

    /// The resolved copy. Placeholder-free on purpose: a shell has no brief to
    /// interpolate — finding out what the user wants is the whole job of the
    /// conversation this message opens.
    ///
    /// `String(localized:)` returns the KEY when the key is absent, so an
    /// unresolved key is silent at build time and at run time. The only guard
    /// is `testTheKickoffMessageResolvesToRealCopy`.
    static var message: String {
        String(localized: String.LocalizationValue(stringLiteral: key))
    }
}

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
    /// Whether the scaffold has landed (protocol 8.0.0's `AppRecordDto.scaffolded`).
    ///
    /// `false` is a SHELL: the record exists and owns a workspace and a pinned
    /// conversation, but `name` is an engine-side placeholder the user never
    /// chose and `brief` is the empty string. Both are decided later, in the
    /// app's own conversation, and only then does `LocalAppScaffold` lay the
    /// scaffold down and flip this to `true`.
    ///
    /// Defaulted to `true` so a fixture or a caller that constructs a summary
    /// without saying anything describes a FORMED app — the shell is the
    /// exceptional state and has to be asked for explicitly.
    var scaffolded: Bool = true

    /// The last host-derived health snapshot for the pinned runtime profile.
    ///
    /// App list records do not carry this detail, so it is populated when a
    /// details snapshot arrives and retained until the next authoritative
    /// details snapshot. `nil` is intentional for shells and for formed apps
    /// whose details have not been loaded yet; the UI must not infer health
    /// from source files or the app's visual/runtime state.
    var runtimeProfileStatus: LocalAppRuntimeProfileStatus? = nil

    /// `true` while this app is still an unscaffolded shell.
    ///
    /// The single predicate every render point branches on, so "does this
    /// place hide the placeholder name?" is one question with one answer
    /// rather than five independent `!scaffolded` spellings that can drift.
    var isDraftShell: Bool { !scaffolded }

    /// The title to put on screen.
    ///
    /// A shell's `name` is the engine's placeholder, so showing it would put a
    /// string the user never chose — and cannot act on — at the top of a card.
    var displayName: String {
        isDraftShell ? String(localized: "local_apps_draft_card_title") : name
    }

    /// The secondary line a shell replaces its normal status line with, or
    /// `nil` once the app is formed and its own status applies.
    ///
    /// Returned as an Optional rather than a plain String so each call site
    /// reads `app.draftStatusLine ?? <its own status>` and cannot forget the
    /// shell case by writing only its own branch.
    var draftStatusLine: String? {
        isDraftShell ? String(localized: "local_apps_draft_card_subtitle") : nil
    }

    /// The brief, or `nil` while this app is a shell.
    ///
    /// A shell's brief is `""` — the engine writes no placeholder for it —
    /// so rendering it produces an empty labelled row, which reads as a bug.
    var displayBrief: String? { isDraftShell ? nil : brief }
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
        case runtimeProfileSelection
        case dependencyChange
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

    var allowsPersistentGrant: Bool {
        switch kind {
        case .runtimeProfileSelection, .dependencyChange:
            false
        default:
            true
        }
    }

    var title: String {
        switch kind {
        case .dataMutation: String(localized: "local_apps_permission_data_mutation")
        case .uiControl: String(localized: "local_apps_permission_ui_control")
        case .networkDomain: String(localized: "local_apps_permission_network")
        case .restoreCheckpoint: String(localized: "local_apps_permission_restore")
        case .runtimeProfileSelection: String(localized: "local_apps_permission_runtime_profile_selection")
        case .dependencyChange: String(localized: "local_apps_permission_dependency_change")
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

enum LocalAppRuntimeProfileFamily: String, CaseIterable, Identifiable, Hashable, Sendable {
    case reactDom
    case canvas2d
    case three3d
    case phaser2d
    case babylon3d

    var id: String { rawValue }

    var title: String {
        switch self {
        case .reactDom: String(localized: "local_apps_runtime_profile_family_react_dom")
        case .canvas2d: String(localized: "local_apps_runtime_profile_family_canvas_2d")
        case .three3d: String(localized: "local_apps_runtime_profile_family_three_3d")
        case .phaser2d: String(localized: "local_apps_runtime_profile_family_phaser_2d")
        case .babylon3d: String(localized: "local_apps_runtime_profile_family_babylon_3d")
        }
    }
}

enum LocalAppRuntimeProfileSurface: Hashable, Sendable {
    case dom
    case canvas

    var title: String {
        switch self {
        case .dom: String(localized: "local_apps_runtime_profile_surface_dom")
        case .canvas: String(localized: "local_apps_runtime_profile_surface_canvas")
        }
    }
}

/// Host-derived health of one app's pinned runtime profile. The raw values
/// are the protocol wire values; keeping them stable lets cache-only cards and
/// detail views share the same finite status vocabulary without parsing host
/// error prose.
enum LocalAppRuntimeProfileStatus: String, CaseIterable, Hashable, Sendable {
    case verified
    case dependenciesDirty = "dependencies_dirty"
    case coreDependencyDrift = "core_dependency_drift"
    case rebuildRequired = "rebuild_required"
    case migrationAvailable = "migration_available"
    case runtimeBundleMissing = "runtime_bundle_missing"
    case runtimeContractCorrupt = "runtime_contract_corrupt"

    var title: String {
        switch self {
        case .verified: String(localized: "local_apps_runtime_profile_health_verified")
        case .dependenciesDirty: String(localized: "local_apps_runtime_profile_health_dependencies_dirty")
        case .coreDependencyDrift: String(localized: "local_apps_runtime_profile_health_core_dependency_drift")
        case .rebuildRequired: String(localized: "local_apps_runtime_profile_health_rebuild_required")
        case .migrationAvailable: String(localized: "local_apps_runtime_profile_health_migration_available")
        case .runtimeBundleMissing: String(localized: "local_apps_runtime_profile_health_runtime_bundle_missing")
        case .runtimeContractCorrupt: String(localized: "local_apps_runtime_profile_health_runtime_contract_corrupt")
        }
    }

    var systemImageName: String {
        switch self {
        case .verified: "checkmark.seal.fill"
        case .dependenciesDirty, .migrationAvailable: "arrow.triangle.2.circlepath"
        case .coreDependencyDrift, .runtimeContractCorrupt: "exclamationmark.shield.fill"
        case .rebuildRequired: "hammer.fill"
        case .runtimeBundleMissing: "shippingbox.fill"
        }
    }
}

struct LocalAppRuntimeProfilePackage: Hashable, Sendable {
    let name: String
    let version: String
}

struct LocalAppRuntimeProfileOption: Identifiable, Hashable, Sendable {
    let family: LocalAppRuntimeProfileFamily
    let revision: UInt32
    let contractSHA256: String
    let surface: LocalAppRuntimeProfileSurface
    let corePackages: [LocalAppRuntimeProfilePackage]
    let cacheStatus: String
    let downloadStatus: String
    let available: Bool
    let reason: String?

    var id: String { family.rawValue }
}

struct LocalAppRuntimeProfileSelectionPrompt: Identifiable, Hashable, Sendable {
    let id: String
    let appID: String
    let reason: String
    let recommendedFamily: LocalAppRuntimeProfileFamily?
    let options: [LocalAppRuntimeProfileOption]
}

enum LocalAppDependencyChangeKind: String, Hashable, Sendable {
    case add
    case update
    case remove

    var title: String {
        switch self {
        case .add: String(localized: "local_apps_dependency_change_add")
        case .update: String(localized: "local_apps_dependency_change_update")
        case .remove: String(localized: "local_apps_dependency_change_remove")
        }
    }
}

struct LocalAppDependencyChange: Hashable, Sendable {
    let kind: LocalAppDependencyChangeKind
    let package: String
    let version: String?
    let cacheStatus: String
    let downloadStatus: String
}

struct LocalAppDependencyChangeConfirmationPrompt: Identifiable, Hashable, Sendable {
    let id: String
    let appID: String
    let reason: String
    let changes: [LocalAppDependencyChange]
    let licenseRisk: String
    let sbomRisk: String
    let lifecycleScriptsBlocked: Bool
    let nativeAddonsBlocked: Bool
    let rollbackPolicy: String
}
