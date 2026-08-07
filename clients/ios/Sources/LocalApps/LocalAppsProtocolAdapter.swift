import Foundation

#if canImport(engine_mobileFFI)
enum LocalAppsProtocolAdapter {
    static func template(_ dto: AppTemplateDto) -> LocalAppTemplate {
        let kind = templateKind(dto.kind)
        return LocalAppTemplate(
            id: kind.rawValue,
            kind: kind,
            version: UInt64(dto.version),
            name: dto.name,
            description: dto.description,
            steps: dto.steps.map(designStep),
            collections: dto.collections.map(collection)
        )
    }

    static func designStep(_ dto: AppDesignStepDto) -> LocalAppDesignStep {
        LocalAppDesignStep(
            id: dto.id,
            order: Int(dto.order),
            title: dto.title,
            description: dto.description ?? "",
            fields: dto.fields.map(designField)
        )
    }

    static func designField(_ dto: AppDesignFieldDto) -> LocalAppDesignField {
        LocalAppDesignField(
            id: dto.id,
            label: dto.label,
            description: dto.description ?? "",
            type: designFieldType(dto.fieldType),
            required: dto.required,
            defaultValue: dto.defaultValue.map(designValue),
            options: dto.options.map {
                LocalAppDesignOption(value: $0.value, label: $0.label)
            }
        )
    }

    static func designFieldType(_ dto: AppDesignFieldTypeDto) -> LocalAppFieldType {
        switch dto {
        case .shortText: .shortText
        case .longText: .longText
        case .singleChoice: .singleChoice
        case .multipleChoice: .multipleChoice
        case .boolean: .boolean
        case .color: .color
        case .density: .density
        case .screenList: .screenList
        case .featureList: .featureList
        case .dataFieldList: .dataFieldList
        case .domainList: .domainList
        }
    }

    static func collection(_ dto: AppDataCollectionDto) -> LocalAppCollectionSchema {
        LocalAppCollectionSchema(
            id: dto.id,
            name: dto.label,
            fields: dto.fields.map(dataField),
            enabledByDefault: dto.enabledByDefault
        )
    }

    static func dataField(_ dto: AppDataFieldDto) -> LocalAppDataField {
        LocalAppDataField(
            id: dto.id,
            name: dto.label,
            type: dataFieldType(dto.fieldType),
            required: dto.required,
            options: dto.options
        )
    }

    static func dataFieldType(_ dto: AppDataFieldTypeDto) -> LocalAppDataFieldType {
        switch dto {
        case .text: .text
        case .longText: .longText
        case .integer: .integer
        case .decimal: .decimal
        case .boolean: .boolean
        case .dateTime: .dateTime
        case .enum: .enumeration
        case .imageRef: .imageReference
        }
    }

    static func dataFieldType(_ value: LocalAppDataFieldType) -> AppDataFieldTypeDto {
        switch value {
        case .text: .text
        case .longText: .longText
        case .integer: .integer
        case .decimal: .decimal
        case .boolean: .boolean
        case .dateTime: .dateTime
        case .enumeration: .enum
        case .imageReference: .imageRef
        }
    }

    static func app(_ dto: AppRecordDto) -> LocalAppSummary {
        LocalAppSummary(
            id: dto.id,
            name: dto.name,
            templateKind: templateKind(dto.template),
            updatedAt: Date(timeIntervalSince1970: TimeInterval(dto.updatedAtMs) / 1_000),
            workflow: workflow(dto.workflowState),
            workspaceRelativePath: dto.workspaceRel
        )
    }

    static func templateKind(_ dto: AppTemplateKindDto) -> LocalAppTemplateKind {
        switch dto {
        case .dashboard: .dashboard
        case .crudTracker: .crudTracker
        case .contentShowcase: .contentShowcase
        case .formUtility: .formUtility
        }
    }

    static func templateKind(_ value: LocalAppTemplateKind) -> AppTemplateKindDto {
        switch value {
        case .dashboard: .dashboard
        case .crudTracker: .crudTracker
        case .contentShowcase: .contentShowcase
        case .formUtility: .formUtility
        }
    }

    static func workflow(_ dto: AppWorkflowStateDto) -> LocalAppWorkflow {
        switch dto {
        // TODO(local-apps#questionnaire, Task 13/18): `authoringQuestionnaire` /
        // `questionnaireFailed` / `planning` / `planFailed` are new
        // conversational-design states (core Task 3) with no dedicated iOS
        // UI yet. Until Task 13/18 add real screens for them, they map onto
        // the closest existing bucket by KIND (an LLM round trip in
        // progress → `.generating`'s busy/spinner treatment; a failed round
        // trip → `.generationFailed`'s retry treatment) — explicit arms,
        // not a catch-all, so this switch still breaks the moment a real
        // state is removed or renamed.
        case .authoringQuestionnaire: .generating
        case .questionnaireFailed: .generationFailed
        case .collectingSpec: .collectingSpec
        case .planning: .generating
        case .planFailed: .generationFailed
        case .awaitingSpecConfirmation: .awaitingSpecConfirmation
        case .generating: .generating
        case .validating: .validating
        case .awaitingPreviewConfirmation: .awaitingPreviewConfirmation
        case .revising: .revising
        case .ready: .ready
        case .generationFailed: .generationFailed
        case .validationFailed: .validationFailed
        }
    }

    static func designValue(_ dto: DesignValueDto) -> LocalAppDesignValue {
        switch dto {
        case let .shortText(value), let .longText(value), let .singleChoice(value):
            .text(value)
        case let .multipleChoice(value), let .screenList(value), let .featureList(value):
            .strings(value)
        case let .boolean(value):
            .boolean(value)
        case let .color(value):
            .color(value)
        case let .density(value):
            .density(value == .compact ? "compact" : "comfortable")
        case let .dataFieldList(value):
            .dataFields(value.map(dataField))
        case let .domainList(value):
            .domains(value)
        // TODO(local-apps#questionnaire, Task 13/14): `.deferred` ("let the
        // model decide") has no dedicated `LocalAppDesignValue` case yet — no
        // iOS designer surface renders it. Map onto an empty text value as a
        // safe, non-crashing placeholder (never a bare catch-all, so this
        // switch still breaks the moment a real case is added or removed)
        // until Task 14 gives it a real "let the model decide" UI.
        case .deferred:
            .text("")
        }
    }

    static func designValue(_ value: LocalAppDesignValue, fieldType: LocalAppFieldType) -> DesignValueDto? {
        switch (fieldType, value) {
        case (.shortText, let .text(value)): .shortText(value: value)
        case (.longText, let .text(value)): .longText(value: value)
        case (.singleChoice, let .text(value)): .singleChoice(value: value)
        case (.multipleChoice, let .strings(value)): .multipleChoice(value: value)
        case (.boolean, let .boolean(value)): .boolean(value: value)
        case (.color, let .color(value)): .color(value: value)
        case (.density, let .density(value)): .density(value: value == "compact" ? .compact : .comfortable)
        case (.screenList, let .strings(value)): .screenList(value: value)
        case (.featureList, let .strings(value)): .featureList(value: value)
        case (.domainList, let .domains(value)): .domainList(value: value)
        case (.dataFieldList, let .dataFields(fields)):
            .dataFieldList(value: fields.map {
                AppDataFieldDto(
                    id: $0.id,
                    label: $0.name,
                    fieldType: dataFieldType($0.type),
                    required: $0.required,
                    options: $0.options
                )
            })
        default: nil
        }
    }

    static func patchChanges(
        _ patch: AppDesignPatchDto,
        fields: [String: LocalAppDesignValue]
    ) -> [LocalAppFieldChange] {
        patch.ops.map { operation in
            switch operation {
            case let .set(fieldId, value):
                LocalAppFieldChange(
                    fieldID: fieldId,
                    oldValue: fields[fieldId],
                    newValue: designValue(value)
                )
            case let .remove(fieldId):
                LocalAppFieldChange(
                    fieldID: fieldId,
                    oldValue: fields[fieldId],
                    newValue: nil
                )
            }
        }
    }

    static func runtime(
        _ dto: AppRuntimeStateDto,
        details: AppRuntimeDetailsDto? = nil,
        lastError: String?,
        knownURL: URL?
    ) -> LocalAppRuntimeStatus {
        let loopbackURL = details?.loopbackUrl.flatMap(URL.init(string:)) ?? knownURL
        if let reason = details?.suspensionReason {
            return .suspended(suspensionReason(reason))
        }
        return switch dto {
        case .stopped: .stopped
        case .starting: .starting
        case .running: .running(loopbackURL)
        case .stopping: .stopping
        case .failed: .failed(details?.lastError ?? lastError ?? String(localized: "local_apps_process_exited"))
        }
    }

    static func suspensionReason(_ reason: AppRuntimeSuspensionReasonDto) -> String {
        switch reason {
        case .backgrounded: String(localized: "local_apps_suspension_background")
        case .memoryWarning: String(localized: "local_apps_suspension_memory")
        case .runtimeQuota: String(localized: "local_apps_suspension_quota")
        case .processExited: String(localized: "local_apps_suspension_exited")
        }
    }

    static func generationStage(_ state: AppGenerationJobStateDto) -> String {
        switch state {
        case .queued: "queued"
        case .scaffolding: "scaffolding"
        case .generating: "generating"
        case .validating: "validating"
        case .building: "building"
        case .startingPreview: "starting_preview"
        case .awaitingApproval: "awaiting_approval"
        case .succeeded: "succeeded"
        case .failed: "failed"
        case .cancelled: "cancelled"
        }
    }

    static func capabilityKind(_ kind: AppCapabilityKindDto) -> LocalAppPermissionPrompt.Kind {
        switch kind {
        case .dataMutation: .dataMutation
        case .uiControl: .uiControl
        case .networkDomain: .networkDomain
        case .restoreCheckpoint: .restoreCheckpoint
        }
    }

    static func uiActionLabel(_ action: AppUiActionKindDto) -> String {
        switch action {
        case .inspect: String(localized: "local_apps_ui_action_inspect")
        case .click: String(localized: "local_apps_ui_action_click")
        case .fill: String(localized: "local_apps_ui_action_fill")
        case .select: String(localized: "local_apps_ui_action_select")
        case .toggle: String(localized: "local_apps_ui_action_toggle")
        case .scroll: String(localized: "local_apps_ui_action_scroll")
        case .navigate: String(localized: "local_apps_ui_action_navigate")
        case .back: String(localized: "local_apps_ui_action_back")
        case .reload: String(localized: "local_apps_ui_action_reload")
        }
    }

    static func authorizationDecision(_ value: LocalAppCapabilityDecision) -> AppAuthorizationDecisionDto {
        switch value {
        case .once: .allowOnce
        case .session: .allowSession
        case .always: .allowAlways
        case .deny: .deny
        }
    }

    static func checkpoint(_ dto: AppCheckpointDto) -> LocalAppCheckpoint {
        LocalAppCheckpoint(
            id: dto.id,
            label: dto.label,
            kind: checkpointKind(dto.kind),
            createdAt: Date(timeIntervalSince1970: TimeInterval(dto.createdAtMs) / 1_000)
        )
    }

    static func checkpointKind(_ kind: AppCheckpointKindDto) -> String {
        switch kind {
        case .scaffoldCreated: "scaffold"
        case .generationValidated: "validated"
        case .previewApproved: "preview"
        case .userApproved: "user"
        case .preRestore: "pre_restore"
        }
    }
}
#endif
