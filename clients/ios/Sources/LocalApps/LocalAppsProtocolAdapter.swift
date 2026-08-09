import Foundation

#if canImport(engine_mobileFFI)
enum LocalAppsProtocolAdapter {
    // NOTE (local-apps#questionnaire, Task 13): `AppTemplateDto`/`AppTemplateKindDto`
    // and the static template catalog they backed were deleted from
    // client-protocol back in Task 5 (human-partner ruling: total removal).
    // `LocalAppTemplate`/`LocalAppTemplateKind` (the native UI models) and the
    // template-picker create screen are deleted here too; the questionnaire/plan
    // this file now maps replace them.

    static func designStep(_ dto: AppDesignStepDto) -> LocalAppDesignStep {
        LocalAppDesignStep(
            id: dto.id,
            order: Int(dto.order),
            title: dto.title,
            description: dto.description ?? "",
            fields: dto.fields.map(designField)
        )
    }

    /// The LLM-authored questionnaire, ordered the way `LocalAppTemplate.orderedSteps`
    /// used to order the static catalog's steps: by declared order, id as tiebreak.
    static func questionnaire(_ steps: [AppDesignStepDto]) -> [LocalAppDesignStep] {
        steps.map(designStep).sorted { lhs, rhs in
            lhs.order == rhs.order ? lhs.id < rhs.id : lhs.order < rhs.order
        }
    }

    static func designField(_ dto: AppDesignFieldDto) -> LocalAppDesignField {
        LocalAppDesignField(
            id: dto.id,
            label: dto.label,
            description: dto.description ?? "",
            type: designFieldType(dto.fieldType),
            required: dto.required,
            allowsCustom: dto.allowsCustom,
            allowsDefer: dto.allowsDefer,
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

    static func collection(_ dto: AppDataCollectionDto) -> LocalAppDataCollection {
        LocalAppDataCollection(
            id: dto.id,
            label: dto.label,
            fields: dto.fields.map(dataField),
            enabledByDefault: dto.enabledByDefault
        )
    }

    static func dataField(_ dto: AppDataFieldDto) -> LocalAppDataField {
        LocalAppDataField(
            id: dto.id,
            label: dto.label,
            fieldType: dataFieldType(dto.fieldType),
            required: dto.required,
            options: dto.options
        )
    }

    static func planCapability(_ dto: AppCapabilityKindDto) -> LocalAppCapabilityKind {
        switch dto {
        case .dataMutation: .dataMutation
        case .uiControl: .uiControl
        case .networkDomain: .networkDomain
        case .restoreCheckpoint: .restoreCheckpoint
        case .camera: .camera
        case .photoLibrary: .photoLibrary
        case .microphone: .microphone
        case .location: .location
        case .notifications: .notifications
        case .llm: .llm
        case .agentNotify: .agentNotify
        }
    }

    static func plan(_ dto: AppPlanDto) -> LocalAppPlan {
        LocalAppPlan(
            collections: dto.collections.map(collection),
            capabilities: dto.capabilities.map(planCapability),
            domains: dto.domains,
            summary: dto.summary
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
            brief: dto.brief,
            updatedAt: Date(timeIntervalSince1970: TimeInterval(dto.updatedAtMs) / 1_000),
            workflow: workflow(dto.workflowState),
            workspaceRelativePath: dto.workspaceRel
        )
    }

    static func workflow(_ dto: AppWorkflowStateDto) -> LocalAppWorkflow {
        switch dto {
        // (local-apps#questionnaire, Task 14, closing the Task 13-review
        // TODO that lived here): `authoringQuestionnaire`/`questionnaireFailed`/
        // `planning`/`planFailed` used to collapse onto `.generating`/
        // `.generationFailed` by KIND. That collapse was more than a display
        // nicety — it let `LocalAppDesignerView.prepare()`'s `.generationFailed`
        // arm call `store.openDesigner(appID:)` for an app that was actually in
        // `questionnaire_failed`/`plan_failed`, neither of which `open_designer`
        // accepts (state.rs:491-501), so the call failed server-side. Each DTO
        // case now maps 1:1 onto its own `LocalAppWorkflow` case instead.
        case .authoringQuestionnaire: .authoringQuestionnaire
        case .questionnaireFailed: .questionnaireFailed
        case .collectingSpec: .collectingSpec
        case .planning: .planning
        case .planFailed: .planFailed
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
        case .deferred:
            .deferred
        }
    }

    static func designValue(_ value: LocalAppDesignValue, fieldType: LocalAppFieldType) -> DesignValueDto? {
        // `.deferred` ("let the model decide") is legal on any field the
        // questionnaire marks `allowsDefer`, regardless of that field's
        // declared type — it carries no payload, so the type-specific
        // dispatch below does not apply to it. Checked first so it always
        // wins over the `(fieldType, value)` match.
        if case .deferred = value { return .deferred }
        // A `return` is required on every arm below now that the function
        // body has more than one statement — Swift only treats a `switch`'s
        // per-case trailing expressions as implicit returns when the switch
        // is the SOLE statement in the body (SE-0380). The `.deferred`
        // early-return above ends that, so without `return` each case here
        // is parsed as a discarded expression statement and `.shortText(...)`
        // etc. fail to type-check for lack of a contextual type.
        switch (fieldType, value) {
        case (.shortText, let .text(value)): return .shortText(value: value)
        case (.longText, let .text(value)): return .longText(value: value)
        case (.singleChoice, let .text(value)): return .singleChoice(value: value)
        case (.multipleChoice, let .strings(value)): return .multipleChoice(value: value)
        case (.boolean, let .boolean(value)): return .boolean(value: value)
        case (.color, let .color(value)): return .color(value: value)
        case (.density, let .density(value)): return .density(value: value == "compact" ? .compact : .comfortable)
        case (.screenList, let .strings(value)): return .screenList(value: value)
        case (.featureList, let .strings(value)): return .featureList(value: value)
        case (.domainList, let .domains(value)): return .domainList(value: value)
        case (.dataFieldList, let .dataFields(fields)):
            return .dataFieldList(value: fields.map {
                AppDataFieldDto(
                    id: $0.id,
                    label: $0.label,
                    fieldType: dataFieldType($0.fieldType),
                    required: $0.required,
                    options: $0.options
                )
            })
        default: return nil
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
        case .camera: .camera
        case .photoLibrary: .photoLibrary
        case .microphone: .microphone
        case .location: .location
        case .notifications: .notifications
        case .llm: .llm
        case .agentNotify: .agentNotify
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
