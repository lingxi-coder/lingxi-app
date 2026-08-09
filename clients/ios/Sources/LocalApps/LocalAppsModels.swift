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
    /// One-line description the user gave at creation time — the seed the
    /// LLM authors the questionnaire from. Replaces `templateKind`
    /// (local-apps#questionnaire, Task 13): there is no more static template
    /// catalog to classify an app by.
    var brief: String
    var updatedAt: Date
    var workflow: LocalAppWorkflow
    var workspaceRelativePath: String
}

enum LocalAppWorkflow: String, CaseIterable, Hashable, Sendable {
    /// The LLM is authoring the questionnaire from the app's brief
    /// (`AppWorkflowState::AuthoringQuestionnaire`). Was collapsed into
    /// `.generationFailed`'s sibling `.generating` bucket until Task 14 gave
    /// it its own case (local-apps#questionnaire, Task 3/13/14 review
    /// Critical residual) — the collapse is what let `LocalAppDesignerView`
    /// fall into `openDesigner` for a state `open_designer` does not accept.
    case authoringQuestionnaire
    /// Questionnaire authoring failed (`AppWorkflowState::QuestionnaireFailed`).
    /// Recoverable via `LocalAppsStore.retryQuestionnaire(appID:)` or
    /// `updateBrief(appID:brief:)` — both already legal from this state
    /// (state.rs `retry_questionnaire`/`update_brief`).
    case questionnaireFailed
    case collectingSpec
    /// The LLM is deriving the plan from the collected answers
    /// (`AppWorkflowState::Planning`).
    case planning
    /// Planning failed (`AppWorkflowState::PlanFailed`). Recoverable via
    /// `LocalAppsStore.retryPlan(appID:)` (`retry_plan`, same answers).
    case planFailed
    case awaitingSpecConfirmation
    case generating
    case validating
    case awaitingPreviewConfirmation
    case revising
    case ready
    case generationFailed
    case validationFailed

    var label: String {
        switch self {
        case .authoringQuestionnaire: String(localized: "local_apps_workflow_authoring_questionnaire")
        case .questionnaireFailed: String(localized: "local_apps_workflow_questionnaire_failed")
        case .collectingSpec: String(localized: "local_apps_workflow_collecting_spec")
        case .planning: String(localized: "local_apps_workflow_planning")
        case .planFailed: String(localized: "local_apps_workflow_plan_failed")
        case .awaitingSpecConfirmation: String(localized: "local_apps_workflow_awaiting_spec")
        case .generating: String(localized: "local_apps_workflow_generating")
        case .validating: String(localized: "local_apps_workflow_validating")
        case .awaitingPreviewConfirmation: String(localized: "local_apps_workflow_awaiting_preview")
        case .revising: String(localized: "local_apps_workflow_revising")
        case .ready: String(localized: "local_apps_workflow_ready")
        case .generationFailed: String(localized: "local_apps_workflow_generation_failed")
        case .validationFailed: String(localized: "local_apps_workflow_validation_failed")
        }
    }

    var isBusy: Bool {
        switch self {
        case .authoringQuestionnaire, .planning, .generating, .validating, .revising: true
        default: false
        }
    }
}

enum LocalAppFieldType: Hashable, Sendable {
    case shortText
    case longText
    case singleChoice
    case multipleChoice
    case boolean
    case color
    case density
    case screenList
    case featureList
    case dataFieldList
    case domainList
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

enum LocalAppDesignValue: Hashable, Sendable {
    case text(String)
    case strings([String])
    case boolean(Bool)
    case color(String)
    case density(String)
    case dataFields([LocalAppDataField])
    case domains([String])
    /// The user explicitly chose to let the LLM decide this field
    /// ("由你决定"). This is an ANSWER, not an absence — the core treats it
    /// as satisfying a required field (mirrors `DesignValueDto.deferred`,
    /// local-apps#questionnaire, Task 1/2/13). Never collapse this to `nil`
    /// or an empty string: that erases the distinction between "unanswered"
    /// and "deferred to the model" the whole feature depends on.
    case deferred

    var textValue: String {
        switch self {
        case let .text(value), let .color(value), let .density(value): value
        case let .strings(values), let .domains(values): values.joined(separator: "\n")
        case let .boolean(value): value ? String(localized: "common_yes") : String(localized: "common_no")
        case let .dataFields(fields): fields.map(\.label).joined(separator: "、")
        case .deferred: String(localized: "local_apps_value_deferred")
        }
    }
}

struct LocalAppDesignField: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let description: String
    let type: LocalAppFieldType
    let required: Bool
    /// Renders an `Other…` free-text box (mirrors `AppDesignFieldDto.allowsCustom`).
    let allowsCustom: Bool
    /// Renders "let the model decide" (mirrors `AppDesignFieldDto.allowsDefer`).
    let allowsDefer: Bool
    let defaultValue: LocalAppDesignValue?
    let options: [LocalAppDesignOption]
}

struct LocalAppDesignOption: Identifiable, Hashable, Sendable {
    var id: String { value }
    let value: String
    let label: String
}

struct LocalAppDesignStep: Identifiable, Hashable, Sendable {
    let id: String
    let order: Int
    let title: String
    let description: String
    let fields: [LocalAppDesignField]
}

struct LocalAppDataCollection: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let fields: [LocalAppDataField]
    let enabledByDefault: Bool
}

/// One capability kind the plan asks the user to grant, mirrors
/// `AppCapabilityKindDto`. Distinct from `LocalAppCapabilityDecision`
/// (once/session/always/deny), which is the user's ANSWER to a capability
/// prompt, not the capability itself.
enum LocalAppCapabilityKind: Hashable, Sendable {
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
}

/// The LLM-derived plan awaiting confirmation (local-apps#questionnaire, Task
/// 1/13). Replaces the deleted `LocalAppTemplate`: a template was a
/// human-authored, static catalog entry; a plan is authored per-app from the
/// questionnaire answers, and voided the moment an answer changes underneath
/// it (see `LocalAppsStore.handle`'s `.appPlanChanged` arm).
struct LocalAppPlan: Hashable, Sendable {
    var collections: [LocalAppDataCollection]
    var capabilities: [LocalAppCapabilityKind]
    /// External HTTPS host names the app may request.
    var domains: [String]
    /// Human-readable summary, including what every deferred field was
    /// finally decided as.
    var summary: String
}

struct LocalAppSuggestionDiff: Identifiable, Hashable, Sendable {
    let id: String
    let summary: String
    let basedOnRevision: UInt64
    let changes: [LocalAppFieldChange]
}

struct LocalAppFieldChange: Identifiable, Hashable, Sendable {
    var id: String { fieldID }
    let fieldID: String
    let oldValue: LocalAppDesignValue?
    let newValue: LocalAppDesignValue?
}

struct LocalAppDesignerSession: Hashable, Sendable {
    let appID: String
    var revision: UInt64
    var interactionID: String?
    var fields: [String: LocalAppDesignValue]
    var currentStep: Int
}

struct LocalAppPreviewSession: Hashable, Sendable {
    let appID: String
    let revision: UInt64
    let interactionID: String
    let url: URL?
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

struct LocalAppGenerationProgress: Hashable, Sendable {
    let stage: String
    let percent: UInt8?
    let detail: String?
}

/// One run of live output from a generation stage.
///
/// Blocks are append-only and coalesced by kind: consecutive chunks of the same
/// kind grow one block rather than creating a new one, so the transcript reads
/// as continuous prose instead of as a list of network packets.
struct LocalAppTranscriptBlock: Identifiable, Hashable, Sendable {
    /// What this run of output is. The engine marks each chunk via the
    /// progress event's `stage`; anything else is a pipeline stage, not a
    /// chunk, and never becomes a block.
    enum Kind: Hashable, Sendable {
        /// Extended-thinking output — the model reasoning about the task.
        case thinking
        /// Assistant text.
        case text

        /// Engine `stage` values that mark a live chunk. Kept as an
        /// initializer (rather than a comparison at the call site) so the two
        /// wire strings appear exactly once on this side of the boundary.
        ///
        /// Mirrors `local_apps_delta::{STAGE_THINKING, STAGE_TEXT}`.
        init?(stage: String) {
            switch stage {
            case "llm_thinking": self = .thinking
            case "llm_text": self = .text
            default: return nil
            }
        }
    }

    /// Position in the transcript. Stable across appends, so SwiftUI keeps the
    /// row identity while the last block grows.
    let id: Int
    let kind: Kind
    var text: String
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
