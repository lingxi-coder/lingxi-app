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
        case .store: "静态运行"
        case .full: "Next 本地服务"
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
        case .collectingSpec: "设计中"
        case .awaitingSpecConfirmation: "等待确认设计"
        case .generating: "正在生成"
        case .validating: "正在校验"
        case .awaitingPreviewConfirmation: "等待批准预览"
        case .revising: "正在修改"
        case .ready: "可运行"
        case .generationFailed: "生成失败"
        case .validationFailed: "校验失败"
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
        case let .boolean(value): value ? "是" : "否"
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
        case .stopped: "已停止"
        case .starting: "正在启动"
        case .running: "运行中"
        case let .suspended(reason): reason.map { "已挂起 · \($0)" } ?? "已挂起"
        case .stopping: "正在停止"
        case let .failed(reason): "运行失败 · \(reason)"
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
        case .once: "允许一次"
        case .session: "本次会话"
        case .always: "始终允许"
        case .deny: "拒绝"
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
        case .dataMutation: "允许 Agent 修改应用数据？"
        case .uiControl: "允许 Agent 控制应用界面？"
        case .networkDomain: "允许应用访问网络？"
        case .restoreCheckpoint: "允许恢复代码检查点？"
        case let .uiAction(action): "允许界面操作：\(action)？"
        }
    }
}
