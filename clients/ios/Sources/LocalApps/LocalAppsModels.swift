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

enum LocalAppTemplateKind: String, CaseIterable, Hashable, Sendable {
    case dashboard
    case crudTracker = "crud_tracker"
    case contentShowcase = "content_showcase"
    case formUtility = "form_utility"

    var systemImage: String {
        switch self {
        case .dashboard: "chart.xyaxis.line"
        case .crudTracker: "checklist"
        case .contentShowcase: "rectangle.grid.2x2"
        case .formUtility: "text.badge.checkmark"
        }
    }
}

struct LocalAppSummary: Identifiable, Hashable, Sendable {
    let id: String
    var name: String
    var templateKind: LocalAppTemplateKind
    var updatedAt: Date
    var workflow: LocalAppWorkflow
    var workspaceRelativePath: String
}

enum LocalAppWorkflow: String, Hashable, Sendable {
    case collectingSpec
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
        case .collectingSpec: String(localized: "local_apps_workflow_collecting_spec")
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
        case .generating, .validating, .revising: true
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
    var name: String
    var type: LocalAppDataFieldType
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

    var textValue: String {
        switch self {
        case let .text(value), let .color(value), let .density(value): value
        case let .strings(values), let .domains(values): values.joined(separator: "\n")
        case let .boolean(value): value ? String(localized: "common_yes") : String(localized: "common_no")
        case let .dataFields(fields): fields.map(\.name).joined(separator: "、")
        }
    }
}

struct LocalAppDesignField: Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let description: String
    let type: LocalAppFieldType
    let required: Bool
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

struct LocalAppCollectionSchema: Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let fields: [LocalAppDataField]
    let enabledByDefault: Bool
}

struct LocalAppTemplate: Identifiable, Hashable, Sendable {
    let id: String
    let kind: LocalAppTemplateKind
    let version: UInt64
    let name: String
    let description: String
    let steps: [LocalAppDesignStep]
    let collections: [LocalAppCollectionSchema]

    var orderedSteps: [LocalAppDesignStep] {
        steps.sorted { lhs, rhs in
            lhs.order == rhs.order ? lhs.id < rhs.id : lhs.order < rhs.order
        }
    }
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
        case let .uiAction(action): String(localized: "local_apps_permission_ui_action \(action)")
        }
    }
}
